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
//! A second, finer map, the shade map, holds the cloud's density through the middle of its
//! layer, which is where every shadow and reflection looks the cloud up. Working that out
//! took each fragment two more octaves of hashed noise (the billows at the cloud's edge)
//! besides the body: at 2268×1473 under broken cloud, the shade map takes 0.22 ms off the
//! terrain's pass and 0.19 ms off the water's, and costs 0.025 ms, read from the body map.
//! It is centred where the ground around the eye takes its shadows from: the cloud towards
//! the light, above it.
//!
//! The maps' places go to the shaders in the atmosphere uniform (`cloud_map` and
//! `cloud_shade`); pipelines that read them set `CLOUD_MAP` and bind them at group 2,
//! bindings 8 and 10, with the sampler at 9. The resource also holds the cloud noise's
//! lattices as textures, which pipelines that set `NOISE_TEXTURES` bind at 11 to 13.

use bevy::asset::embedded_asset;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    AddressMode, BindGroup, BindGroupEntries, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    BindGroupLayoutEntry, BindingResource, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState, LoadOp, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType,
    SamplerDescriptor, ShaderStages, StoreOp, TextureDataOrder, TextureDescriptor, TextureDimension, TextureFormat,
    TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor,
    binding_types::{sampler as sampler_binding, texture_2d, texture_3d, uniform_buffer},
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::{RenderApp, RenderStartup};

use super::atmosphere::{Atmosphere, AtmosphereBuffer};

const SHADER: &str = "embedded://voxmar/render/cloud_map.wgsl";
/// Texels across the map, and blocks it spans: 12.5 blocks a texel, finer than the body's
/// smallest octave (about 60 blocks), out past the farthest cloud drawn (6000 blocks).
pub const SIZE: u32 = 1024;
pub const EXTENT: f32 = 12_800.0;
/// The shade map's texels across and blocks spanned: 4 blocks a texel, a quarter of the
/// cloud's finest detail (about 17 blocks), and out past where the loaded chunks take their
/// shadows from but for a low sun.
pub const SHADE_SIZE: u32 = 512;
pub const SHADE_EXTENT: f32 = 2048.0;
const FORMAT: TextureFormat = TextureFormat::R16Float;

/// Where the map's corner lies from the eye at `eye` (x and z), in whole texels of the
/// world, so the texels stay put as the eye moves.
pub fn corner(eye: DVec2) -> Vec2 {
    corner_of(eye, eye, SIZE, EXTENT)
}

/// Where the shade map's corner lies from the eye at `eye` (x and z), around `centre`.
pub fn shade_corner(eye: DVec2, centre: DVec2) -> Vec2 {
    corner_of(eye, centre, SHADE_SIZE, SHADE_EXTENT)
}

fn corner_of(eye: DVec2, centre: DVec2, size: u32, extent: f32) -> Vec2 {
    let texel = extent as f64 / size as f64;
    let corner = ((centre - extent as f64 / 2.0) / texel).floor() * texel;
    (corner - eye).as_vec2()
}

/// Cells before the cloud noise's 2D and 3D lattices repeat (clouds.wgsl).
const NOISE_2D: u32 = 256;
const NOISE_3D: u32 = 64;

/// The cloud noise's lattices as textures, which shaders sample with the hardware's filter
/// instead of hashing every corner (clouds.wgsl, NOISE_TEXTURES): one filtered read for a
/// value noise instead of four or eight hashes.
pub struct CloudNoise {
    pub plane: TextureView,
    pub volume: TextureView,
    pub sampler: Sampler,
}

/// The PCG hash and its 0..1 value, as clouds.wgsl has them.
fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

fn to_byte(v: f32) -> u8 {
    (v * 255.0).round() as u8
}

impl CloudNoise {
    fn new(device: &RenderDevice, queue: &RenderQueue) -> Self {
        let plane: Vec<u8> = (0..NOISE_2D * NOISE_2D)
            .map(|i| to_byte(unit(pcg((i % NOISE_2D).wrapping_add(pcg((i / NOISE_2D) ^ 0x9e37_79b9))))))
            .collect();
        let volume: Vec<u8> = (0..NOISE_3D.pow(3))
            .map(|i| {
                let (x, y, z) = (i % NOISE_3D, i / NOISE_3D % NOISE_3D, i / (NOISE_3D * NOISE_3D));
                to_byte(unit(pcg(x.wrapping_add(pcg(y.wrapping_add(pcg(z ^ 0x2545_f491)))))))
            })
            .collect();
        let texture = |label, size: u32, layers: u32, dimension, data: &[u8]| {
            device
                .create_texture_with_data(
                    queue,
                    &TextureDescriptor {
                        label: Some(label),
                        size: Extent3d { width: size, height: size, depth_or_array_layers: layers },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension,
                        format: TextureFormat::R8Unorm,
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                        view_formats: &[],
                    },
                    TextureDataOrder::LayerMajor,
                    data,
                )
                .create_view(&TextureViewDescriptor::default())
        };
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("cloud_noise"),
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            address_mode_w: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        });
        Self {
            plane: texture("cloud_noise_2d", NOISE_2D, 1, TextureDimension::D2, &plane),
            volume: texture("cloud_noise_3d", NOISE_3D, NOISE_3D, TextureDimension::D3, &volume),
            sampler,
        }
    }
}

/// The bindings in group 2 of the cloud maps (8 and 10, sampled with 9) and of the cloud
/// noise's lattice textures (11 to 13), for every pipeline whose shaders set `CLOUD_MAP` and
/// `NOISE_TEXTURES`: those built like the terrain's (`terrain::mesh_like`) and the
/// volumetric pass. Kept in one place so no pipeline leaves one out: the particles once
/// did, and their first frame failed validation and quit.
pub fn layout_entries(visibility: ShaderStages) -> [BindGroupLayoutEntry; 6] {
    let filterable = TextureSampleType::Float { filterable: true };
    [
        texture_2d(filterable).build(8, visibility),
        sampler_binding(SamplerBindingType::Filtering).build(9, visibility),
        texture_2d(filterable).build(10, visibility),
        texture_2d(filterable).build(11, visibility),
        texture_3d(filterable).build(12, visibility),
        sampler_binding(SamplerBindingType::Filtering).build(13, visibility),
    ]
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
    pub shade: TextureView,
    pub sampler: Sampler,
    pub noise: CloudNoise,
    /// The body map's and the shade map's.
    pipelines: [CachedRenderPipelineId; 2],
    layouts: [BindGroupLayoutDescriptor; 2],
    groups: Option<[BindGroup; 2]>,
}

fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    asset_server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let texture = |label, size| {
        device
            .create_texture(&TextureDescriptor {
                label: Some(label),
                size: Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&TextureViewDescriptor::default())
    };
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("cloud_map"),
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    let layouts = [
        BindGroupLayoutDescriptor::new(
            "cloud_map_layout",
            &BindGroupLayoutEntries::single(ShaderStages::FRAGMENT, uniform_buffer::<Atmosphere>(false)),
        ),
        // The shade map's reads the body map.
        BindGroupLayoutDescriptor::new(
            "cloud_shade_map_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    uniform_buffer::<Atmosphere>(false),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler_binding(SamplerBindingType::Filtering),
                ),
            ),
        ),
    ];
    let shader = asset_server.load(SHADER);
    let pipeline = |label: &'static str, layout: &BindGroupLayoutDescriptor, entry_point: &'static str| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some(entry_point.into()),
                targets: vec![Some(ColorTargetState { format: FORMAT, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    };
    let pipelines =
        [pipeline("cloud_map", &layouts[0], "fragment"), pipeline("cloud_shade_map", &layouts[1], "shade")];
    commands.insert_resource(CloudMap {
        view: texture("cloud_map", SIZE),
        shade: texture("cloud_shade_map", SHADE_SIZE),
        sampler,
        noise: CloudNoise::new(&device, &queue),
        pipelines,
        layouts,
        groups: None,
    });
}

impl CloudMap {
    /// What `layout_entries` lays out.
    pub fn entries(&self) -> [BindGroupEntry<'_>; 6] {
        let entry = |binding, resource| BindGroupEntry { binding, resource };
        [
            entry(8, BindingResource::TextureView(&self.view)),
            entry(9, BindingResource::Sampler(&self.sampler)),
            entry(10, BindingResource::TextureView(&self.shade)),
            entry(11, BindingResource::TextureView(&self.noise.plane)),
            entry(12, BindingResource::TextureView(&self.noise.volume)),
            entry(13, BindingResource::Sampler(&self.noise.sampler)),
        ]
    }
}

/// Draws the maps while there is cloud (`Atmosphere::cloud_map.w`).
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
    let [Some(body), Some(shade)] = map.pipelines.map(|id| cache.get_render_pipeline(id)) else { return };
    if map.groups.is_none() {
        let device = ctx.render_device();
        let groups = [
            device.create_bind_group(
                "cloud_map",
                &cache.get_bind_group_layout(&map.layouts[0]),
                &BindGroupEntries::single(buffer.0.as_entire_binding()),
            ),
            device.create_bind_group(
                "cloud_shade_map",
                &cache.get_bind_group_layout(&map.layouts[1]),
                &BindGroupEntries::sequential((buffer.0.as_entire_binding(), &map.view, &map.sampler)),
            ),
        ];
        map.groups = Some(groups);
    }
    let Some([body_group, shade_group]) = map.groups.as_ref() else { return };
    // Both in one command buffer, the body first.
    let encoder = ctx.command_encoder();
    let passes = [("cloud_map", &map.view, body, body_group), ("cloud_shade_map", &map.shade, shade, shade_group)];
    for (label, view, pipeline, group) in passes {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(RenderPassColorAttachment {
                view,
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
}
