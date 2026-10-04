//! The last steps of a frame before anti-aliasing and the UI: bloom (bloom.wgsl), then the
//! display transform (display.wgsl), which adds the bloom to the HDR image and maps it to
//! the screen.
//!
//! Bloom is Bevy's method (its filters, threshold and energy-conserving blend), but Bevy
//! draws it as a render pass per level of its chain, sixteen a frame. On Metal every pass
//! is a command buffer of its own, and each one costs about 0.2 ms of frame time however
//! little it draws (docs/rendering.md), so ours runs the whole chain as compute dispatches
//! in a single pass, and the display pass adds it to the image on the way to the screen:
//! two passes in place of seventeen.

use bevy::asset::embedded_asset;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::post_process::bloom::bloom;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_storage_2d, uniform_buffer_sized};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferBinding, BufferDescriptor,
    BufferUsages, CachedComputePipelineId, CachedRenderPipelineId, ColorTargetState, ColorWrites, ComputePassDescriptor,
    ComputePipelineDescriptor, Extent3d, FragmentState, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    ShaderType, StorageTextureAccess, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureView, TextureViewDescriptor, UniformBuffer, encase,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use rustc_hash::FxHashMap;

use super::DisplayTransform;

const BLOOM_SHADER: &str = "embedded://voxmar/render/bloom.wgsl";
const DISPLAY_SHADER: &str = "embedded://voxmar/render/display.wgsl";
const CHAIN_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// The chain's first level is this many pixels tall (with the image's aspect), and each
/// level below it half the last, down to a few pixels: Bevy's default.
const CHAIN_HEIGHT: u32 = 512;
const LEVELS: u32 = CHAIN_HEIGHT.ilog2() - 1;
/// Only what is brighter than a sunlit white block blooms (the sun, glints, light
/// sources), so tiny bright facets in the distance never swell into blobs.
const THRESHOLD: f32 = 1.2;
const THRESHOLD_SOFTNESS: f32 = 0.5;
/// How the levels blend, as Bevy's `Bloom::NATURAL`.
const LOW_FREQUENCY_BOOST: f32 = 0.7;
const LOW_FREQUENCY_BOOST_CURVATURE: f32 = 0.95;
const HIGH_PASS_FREQUENCY: f32 = 1.0;
/// Uniform buffer offsets must be aligned to this.
const STEP_STRIDE: u64 = 256;
const WORKGROUP: u32 = 8;

/// How strongly the camera's image blooms, 0 for not at all; the shader pack sets it.
#[derive(Component, ExtractComponent, Clone, Copy, PartialEq)]
pub struct Glow {
    pub intensity: f32,
}

pub struct PostPlugin;

impl Plugin for PostPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "bloom.wgsl");
        app.add_plugins((ExtractComponentPlugin::<Glow>::default(), ExtractComponentPlugin::<DisplayTransform>::default()));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(Core3d, bloom_and_display.in_set(Core3dSystems::PostProcess).after(bloom).before(tonemapping));
    }
}

/// One bloom dispatch's uniforms (bloom.wgsl's `Step`).
#[derive(ShaderType, Clone, Copy, Default)]
struct Step {
    threshold: Vec4,
    blend: f32,
}

/// The display pass's uniforms (display.wgsl's `DisplayTransform`).
#[derive(ShaderType, Clone, Copy, Default)]
struct DisplayUniform {
    exposure: f32,
    contrast: f32,
    saturation: f32,
    vibrance: f32,
    warmth: f32,
    bloom: f32,
}

#[derive(Resource)]
pub struct PostPipelines {
    bloom_layout: BindGroupLayoutDescriptor,
    display_layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    downsample_first: CachedComputePipelineId,
    downsample: CachedComputePipelineId,
    upsample: CachedComputePipelineId,
    display_shader: Handle<Shader>,
    fullscreen: FullscreenShader,
    /// The display pipeline for each target format.
    display: FxHashMap<TextureFormat, CachedRenderPipelineId>,
}

fn init(mut commands: Commands, device: Res<RenderDevice>, asset_server: Res<AssetServer>, cache: Res<PipelineCache>, fullscreen: Res<FullscreenShader>) {
    let bloom_layout = BindGroupLayoutDescriptor::new(
        "bloom_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_storage_2d(CHAIN_FORMAT, StorageTextureAccess::WriteOnly),
                uniform_buffer_sized(false, Some(Step::min_size())),
            ),
        ),
    );
    let display_layout = BindGroupLayoutDescriptor::new(
        "display_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer_sized(false, Some(DisplayUniform::min_size())),
                texture_2d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    );
    let shader = asset_server.load(BLOOM_SHADER);
    let compute = |entry_point: &'static str| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("bloom_{entry_point}").into()),
            layout: vec![bloom_layout.clone()],
            shader: shader.clone(),
            entry_point: Some(entry_point.into()),
            ..default()
        })
    };
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: bevy::render::render_resource::FilterMode::Linear,
        min_filter: bevy::render::render_resource::FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(PostPipelines {
        downsample_first: compute("downsample_first"),
        downsample: compute("downsample"),
        upsample: compute("upsample"),
        bloom_layout,
        display_layout,
        sampler,
        display_shader: asset_server.load(DISPLAY_SHADER),
        fullscreen: fullscreen.clone(),
        display: FxHashMap::default(),
    });
}

impl PostPipelines {
    fn display(&mut self, format: TextureFormat, cache: &PipelineCache) -> CachedRenderPipelineId {
        *self.display.entry(format).or_insert_with(|| {
            cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("display_transform".into()),
                layout: vec![self.display_layout.clone()],
                vertex: self.fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: self.display_shader.clone(),
                    targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                    ..default()
                }),
                ..default()
            })
        })
    }
}

/// How much of the level below an upsample into `level` blends in, as Bevy's
/// `compute_blend_factor` for energy-conserving bloom: more of the blurrier levels the
/// stronger the bloom, and the low frequencies boosted.
fn blend(intensity: f32, level: f32, lowest: f32) -> f32 {
    let x = level / lowest;
    let boost = (1.0 - (1.0 - x).powf(1.0 / (1.0 - LOW_FREQUENCY_BOOST_CURVATURE))) * LOW_FREQUENCY_BOOST * (1.0 - intensity);
    let high_pass = 1.0 - ((x - HIGH_PASS_FREQUENCY) / HIGH_PASS_FREQUENCY).clamp(0.0, 1.0);
    (intensity + boost) * high_pass
}

/// A camera's bloom chain and display uniforms this frame.
#[derive(Component)]
pub struct PostView {
    display_pipeline: CachedRenderPipelineId,
    display_uniform: UniformBuffer<DisplayUniform>,
    /// The levels as downsampled, and as upsampled back up (the top `LEVELS - 1` of them;
    /// the lowest is the downsampled one).
    down: CachedTexture,
    up: CachedTexture,
    steps: Buffer,
    /// Each dispatch after the first: its pipeline, bind group and the size of the level it
    /// writes. The first, which reads the image, is bound in `post`.
    dispatches: Vec<(CachedComputePipelineId, BindGroup, UVec2)>,
    /// What the display pass reads of the bloom: the top level as upsampled.
    top: TextureView,
    bloom: bool,
}

fn level_view(texture: &CachedTexture, level: u32) -> TextureView {
    texture.texture.create_view(&TextureViewDescriptor { base_mip_level: level, mip_level_count: Some(1), ..default() })
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    mut commands: Commands,
    mut pipelines: ResMut<PostPipelines>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut textures: ResMut<TextureCache>,
    mut views: Query<(Entity, &ExtractedView, &ExtractedCamera, Option<&Glow>, Option<&DisplayTransform>, Option<&mut PostView>)>,
) {
    for (entity, view, camera, glow, display, existing) in &mut views {
        let (Some(display), Some(size)) = (display, camera.physical_viewport_size) else {
            commands.entity(entity).remove::<PostView>();
            continue;
        };
        let intensity = glow.map_or(0.0, |g| g.intensity.max(0.0));
        let chain = (size.as_vec2() * CHAIN_HEIGHT as f32 / size.y.max(1) as f32).round().as_uvec2().max(UVec2::ONE);
        let mut texture = |label: &'static str, levels: u32| {
            textures.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d { width: chain.x, height: chain.y, depth_or_array_layers: 1 },
                    mip_level_count: levels,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: CHAIN_FORMAT,
                    usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let (down, up) = (texture("bloom_down", LEVELS), texture("bloom_up", LEVELS - 1));
        let display_pipeline = pipelines.display(view.target_format, &cache);

        // The first dispatch thresholds, the rest blend their level's share.
        let knee = THRESHOLD * THRESHOLD_SOFTNESS;
        let threshold = Vec4::new(THRESHOLD, THRESHOLD - knee, 2.0 * knee, 0.25 / (knee + 0.00001));
        let lowest = (LEVELS - 1) as f32;
        let mut bytes = vec![0u8; (STEP_STRIDE * LEVELS as u64) as usize];
        for level in 0..LEVELS {
            let step = Step { threshold, blend: blend(intensity, level as f32 + 1.0, lowest) };
            let mut uniform = encase::UniformBuffer::new(Vec::<u8>::new());
            uniform.write(&step).expect("a bloom step fits a uniform");
            let at = (STEP_STRIDE * level as u64) as usize;
            bytes[at..at + uniform.as_ref().len()].copy_from_slice(uniform.as_ref());
        }

        let mut display_uniform = UniformBuffer::from(DisplayUniform {
            exposure: display.exposure,
            contrast: display.contrast,
            saturation: display.saturation,
            vibrance: display.vibrance,
            warmth: display.warmth,
            bloom: if intensity > 0.0 { blend(intensity, 0.0, lowest) } else { 0.0 },
        });
        display_uniform.write_buffer(&device, &queue);

        if let Some(mut existing) = existing
            && existing.down.texture.id() == down.texture.id()
            && existing.up.texture.id() == up.texture.id()
        {
            queue.write_buffer(&existing.steps, 0, &bytes);
            existing.display_pipeline = display_pipeline;
            existing.display_uniform = display_uniform;
            existing.bloom = intensity > 0.0;
            continue;
        }

        let steps = device.create_buffer(&BufferDescriptor {
            label: Some("bloom_steps"),
            size: bytes.len() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&steps, 0, &bytes);
        let step = |index: u32| BufferBinding { buffer: &steps, offset: STEP_STRIDE * index as u64, size: Some(Step::min_size()) };
        let layout = cache.get_bind_group_layout(&pipelines.bloom_layout);
        let downs: Vec<TextureView> = (0..LEVELS).map(|level| level_view(&down, level)).collect();
        let ups: Vec<TextureView> = (0..LEVELS - 1).map(|level| level_view(&up, level)).collect();
        let level_size = |level: u32| (chain >> level).max(UVec2::ONE);
        let mut dispatches = Vec::new();
        for level in 1..LEVELS {
            let group = device.create_bind_group(
                "bloom_downsample",
                &layout,
                &BindGroupEntries::sequential((&downs[level as usize - 1], &pipelines.sampler, &downs[level as usize - 1], &downs[level as usize], step(level))),
            );
            dispatches.push((pipelines.downsample, group, level_size(level)));
        }
        // Up from the lowest level: each upsampled level blends the one below it in.
        for level in (0..LEVELS - 1).rev() {
            let below = if level + 1 == LEVELS - 1 { &downs[level as usize + 1] } else { &ups[level as usize + 1] };
            let group = device.create_bind_group(
                "bloom_upsample",
                &layout,
                &BindGroupEntries::sequential((below, &pipelines.sampler, &downs[level as usize], &ups[level as usize], step(level))),
            );
            dispatches.push((pipelines.upsample, group, level_size(level)));
        }
        let top = ups[0].clone();
        commands.entity(entity).insert(PostView {
            display_pipeline,
            display_uniform,
            down,
            up,
            steps,
            dispatches,
            top,
            bloom: intensity > 0.0,
        });
    }
}

/// Runs the bloom chain, then the display transform into the next post-process target.
pub fn bloom_and_display(
    view: ViewQuery<(&ViewTarget, &PostView)>,
    pipelines: Res<PostPipelines>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (target, post) = view.into_inner();
    let Some(display) = cache.get_render_pipeline(post.display_pipeline) else { return };
    let Some(uniform) = post.display_uniform.binding() else { return };

    let first = cache.get_compute_pipeline(pipelines.downsample_first);
    let rest: Option<Vec<_>> = post.dispatches.iter().map(|(id, group, size)| Some((cache.get_compute_pipeline(*id)?, group, *size))).collect();
    if let (true, Some(first), Some(rest)) = (post.bloom, first, rest) {
        let top = level_view(&post.down, 0);
        let first_group = device.create_bind_group(
            "bloom_downsample_first",
            &cache.get_bind_group_layout(&pipelines.bloom_layout),
            &BindGroupEntries::sequential((
                target.main_texture_view(),
                &pipelines.sampler,
                target.main_texture_view(),
                &top,
                BufferBinding { buffer: &post.steps, offset: 0, size: Some(Step::min_size()) },
            )),
        );
        let chain = post.down.texture.size();
        let mut pass = ctx.command_encoder().begin_compute_pass(&ComputePassDescriptor { label: Some("bloom"), timestamp_writes: None });
        pass.set_pipeline(first);
        pass.set_bind_group(0, &first_group, &[]);
        pass.dispatch_workgroups(chain.width.div_ceil(WORKGROUP), chain.height.div_ceil(WORKGROUP), 1);
        for (pipeline, group, size) in rest {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.x.div_ceil(WORKGROUP), size.y.div_ceil(WORKGROUP), 1);
        }
    }

    let write = target.post_process_write();
    let group = device.create_bind_group(
        "display_transform",
        &cache.get_bind_group_layout(&pipelines.display_layout),
        &BindGroupEntries::sequential((write.source, &pipelines.sampler, uniform, &post.top)),
    );
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("display_transform"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: write.destination,
            depth_slice: None,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(display);
    pass.set_bind_group(0, &group, &[]);
    pass.draw(0..3, 0..1);
}
