//! Water, drawn in a pass of its own so it can see what lies behind and around it.
//!
//! Water and ice are the terrain's translucent faces (`TerrainPass::Translucent`). They are
//! drawn after the opaque terrain and before Bevy's transparent pass, with the scene so far
//! bound for reading (`water.wgsl`): its depth, through a read-only depth attachment, so
//! nothing is copied, and its colour, which with MSAA is the resolved image (the pass draws
//! into the multisampled one) and otherwise a copy. From those the water shader finds
//!
//! - how far light travels through the water to whatever is behind it, so shallows are
//!   clear and deep water takes its colour, with no bands at whole blocks;
//! - the bed seen through the surface, bent by the waves;
//! - reflections of the terrain by marching the depth buffer, and of the sky elsewhere;
//! - foam where the surface meets the shore.
//!
//! Waves are octaves of the "very fast procedural ocean" (afl_ext, MIT): their directions,
//! frequencies and phases are worked out here each frame, in double precision around the
//! camera, so far from the origin they stay smooth, and they lean with the weather's wind.

use bevy::core_pipeline::core_3d::{main_opaque_pass_3d, main_transparent_pass_3d};
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::pbr::{SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup, main_transmissive_pass_3d};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_phase::{
    AddRenderCommand, CachedRenderPipelinePhaseItem, DrawFunctionId, DrawFunctions, PhaseItem, PhaseItemExtraIndex,
    SetItemPipeline, ViewSortedRenderPhases,
};
use bevy::render::render_resource::{
    AddressMode, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedRenderPipelineId, Extent3d, FilterMode, LoadOp, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDepthStencilAttachment, RenderPassDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    ShaderType, StoreOp, TextureDescriptor, TextureDimension, TextureSampleType, TextureUsages, encase,
    binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer},
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::core_pipeline::core_3d::Transparent3d;
use bevy::shader::load_shader_library;
use serde::{Deserialize, Serialize};

use super::pack::ActivePack;
use super::terrain::{DrawTerrain, TerrainDraws};
use super::{DayCycle, SkyHandle, SkyMaterial};
use crate::weather::Wind;

/// Wave octaves the shader can sum (`water.wgsl`).
pub const MAX_OCTAVES: usize = 16;

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "water.wgsl");
        load_shader_library!(app, "water_surface.wgsl");
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
                (write_uniform, prepare_scene_copies).in_set(RenderSystems::PrepareResources),
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

    /// Wave octaves summed, steps of the reflection march, and halvings that refine a hit.
    fn budget(self) -> (usize, u32, u32) {
        match self {
            WaterQuality::Low => (0, 0, 0),
            WaterQuality::Medium => (8, 16, 4),
            WaterQuality::High => (MAX_OCTAVES, 28, 6),
        }
    }
}

/// The waves on still water: octaves of `exp(sin x)` waves, each a little shorter, quicker
/// and weaker than the last, in directions scattered by a fixed angle (afl_ext's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Octave {
    /// Unit direction the crests travel in, along x and z.
    pub direction: Vec2,
    /// Radians per block.
    pub frequency: f32,
    /// Radians per second.
    pub speed: f32,
    pub weight: f32,
}

/// Longest waves, in radians per block (about six blocks from crest to crest).
const BASE_FREQUENCY: f64 = 1.0;
const FREQUENCY_GROWTH: f64 = 1.18;
const BASE_SPEED: f64 = 1.6;
const SPEED_GROWTH: f64 = 1.07;
const WEIGHT_DECAY: f32 = 0.8;
/// Turn between successive octaves' directions, in radians (afl_ext's).
const DIRECTION_STEP: f64 = 1232.399963;

/// The first `count` octaves for a wind (blocks per second along x and z): the long waves
/// turn towards the wind's direction the harder it blows, the short ones stay scattered.
/// Weights sum to one.
pub fn octaves(count: usize, wind: Vec2) -> Vec<Octave> {
    let speed = wind.length();
    let along = if speed > 0.01 { wind / speed } else { Vec2::X };
    // A breeze lines up the swell; a gale lines up nearly everything.
    let alignment = (speed / 12.0).clamp(0.0, 0.85);
    let mut octaves: Vec<Octave> = (0..count)
        .map(|i| {
            let angle = DIRECTION_STEP * i as f64;
            let scattered = Vec2::new(angle.sin() as f32, angle.cos() as f32);
            // Each octave keeps the side of the wind it started on, so they never all
            // cancel out, and shorter octaves turn less.
            let pull = alignment * WEIGHT_DECAY.powi(i as i32);
            let toward = if scattered.dot(along) >= 0.0 { along } else { -along };
            let direction = scattered.lerp(toward, pull).normalize_or(scattered);
            Octave {
                direction,
                frequency: (BASE_FREQUENCY * FREQUENCY_GROWTH.powi(i as i32)) as f32,
                speed: (BASE_SPEED * SPEED_GROWTH.powi(i as i32)) as f32,
                weight: WEIGHT_DECAY.powi(i as i32),
            }
        })
        .collect();
    let total: f32 = octaves.iter().map(|o| o.weight).sum();
    for octave in &mut octaves {
        octave.weight /= total.max(f32::EPSILON);
    }
    octaves
}

/// Phase of an octave at a world position and time, wrapped to one turn. Done in double
/// precision, so the shader only adds the small part relative to `origin`.
pub fn phase(octave: &Octave, origin: DVec2, seconds: f64) -> f32 {
    let x = octave.direction.as_dvec2().dot(origin) * octave.frequency as f64 - seconds * octave.speed as f64;
    x.rem_euclid(std::f64::consts::TAU) as f32
}

/// Everything the water shader is given, gathered in the main world each frame.
#[derive(Resource, Clone, ExtractResource, Default)]
pub struct WaterParams {
    uniform: WaterUniform,
}

/// See `Water` in water.wgsl.
#[derive(ShaderType, Clone, Copy, Default)]
struct WaterUniform {
    waves: [Vec4; MAX_OCTAVES],
    weights: [Vec4; MAX_OCTAVES / 4],
    origin: Vec4,
    zenith: Vec4,
    horizon: Vec4,
    sun: Vec4,
    optics: Vec4,
    strengths: Vec4,
    budget: UVec4,
}

/// Wave positions are measured from the camera snapped to this many blocks, so they stay
/// small without moving every frame.
const ORIGIN_SNAP: f64 = 256.0;

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
    let (count, steps, refinements) = quality.budget();
    let at = camera.translation().as_dvec3();
    let origin = (DVec2::new(at.x, at.z) / ORIGIN_SNAP).floor() * ORIGIN_SNAP;
    let seconds = time.elapsed_secs_f64();
    let octaves = octaves(count, wind.mean());
    let mut uniform = WaterUniform::default();
    for (i, octave) in octaves.iter().enumerate() {
        uniform.waves[i] = octave.direction.extend(octave.frequency).extend(phase(octave, origin, seconds));
        uniform.weights[i / 4][i % 4] = octave.weight;
    }
    // Calm water still ripples; wind raises the waves up to a strong breeze.
    let roughness = 0.45 + 0.55 * (wind.mean().length() / 8.0).min(1.0);
    uniform.origin = Vec4::new(origin.x as f32, 0.0, origin.y as f32, look.waves * roughness);
    if let Some(sky) = sky_handle.and_then(|handle| sky_materials.get(&handle.0)) {
        uniform.zenith = sky.params.zenith;
        uniform.horizon = sky.params.horizon;
        uniform.sun = sky.params.sun;
    }
    uniform.optics = Vec4::new(look.absorption, look.scattering, look.refraction, if cycle.underwater { 1.0 } else { 0.0 });
    uniform.strengths = Vec4::new(look.reflections, look.foam, if look.pixelated { 1.0 } else { 0.0 }, 0.0);
    uniform.budget = UVec4::new(octaves.len() as u32, steps, refinements, 0);
    params.uniform = uniform;
}

/// The water bind group's layout (group 3 of the translucent terrain pipeline): the scene's
/// depth and colour, a sampler and the uniform.
pub fn layout(multisampled: bool) -> BindGroupLayoutDescriptor {
    let entries = if multisampled {
        BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d_multisampled(),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<WaterUniform>(false),
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
            ),
        )
    };
    BindGroupLayoutDescriptor::new(if multisampled { "water_layout_msaa" } else { "water_layout" }, &entries)
}

#[derive(Resource)]
struct WaterGpu {
    uniform: Buffer,
    sampler: Sampler,
}

fn init_gpu(mut commands: Commands, device: Res<RenderDevice>) {
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
    commands.insert_resource(WaterGpu { uniform, sampler });
}

fn write_uniform(params: Res<WaterParams>, gpu: Option<Res<WaterGpu>>, queue: Res<RenderQueue>) {
    let Some(gpu) = gpu else { return };
    let mut buffer = encase::UniformBuffer::new(Vec::<u8>::new());
    buffer.write(&params.uniform).expect("the water uniform fits");
    queue.write_buffer(&gpu.uniform, 0, buffer.as_ref());
}

/// Without MSAA the image water is drawn into is the one it must read, so the scene so far
/// is copied for it.
#[derive(Component)]
struct SceneCopy(CachedTexture);

fn prepare_scene_copies(
    mut commands: Commands,
    mut cache: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera, &ExtractedView, &Msaa, Option<&TerrainDraws>)>,
) {
    for (entity, camera, view, msaa, draws) in &views {
        let wanted = *msaa == Msaa::Off && draws.is_some_and(TerrainDraws::has_water);
        let Some(size) = camera.physical_target_size.filter(|_| wanted) else {
            commands.entity(entity).remove::<SceneCopy>();
            continue;
        };
        let texture = cache.get(
            &device,
            TextureDescriptor {
                label: Some("water_scene_copy"),
                size: Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: view.target_format,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
        );
        commands.entity(entity).insert(SceneCopy(texture));
    }
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

/// The translucent terrain's draw commands; `water_pass` binds group 3 itself.
type DrawWater = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawTerrain);

/// Draws the translucent terrain of a camera with the scene so far bound for reading. With
/// MSAA it draws into the multisampled image and reads the resolved one, then resolves
/// again unless Bevy's transparent pass, which follows, will.
#[allow(clippy::too_many_arguments)]
fn water_pass(
    world: &World,
    view: ViewQuery<(
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthTexture,
        &Msaa,
        &TerrainDraws,
        Option<&SceneCopy>,
    )>,
    gpu: Option<Res<WaterGpu>>,
    pipeline_cache: Res<PipelineCache>,
    transparent_phases: Res<ViewSortedRenderPhases<Transparent3d>>,
    mut ctx: RenderContext,
) {
    let view_entity = view.entity();
    let (camera, extracted, target, depth, msaa, draws, copy) = view.into_inner();
    let (Some(gpu), Some(water)) = (gpu, draws.water()) else { return };
    let multisampled = *msaa != Msaa::Off;
    let layout = pipeline_cache.get_bind_group_layout(&layout(multisampled));
    let scene = if multisampled {
        target.main_texture_view()
    } else {
        let Some(SceneCopy(copy)) = copy else { return };
        let Some(size) = camera.physical_target_size else { return };
        ctx.command_encoder().copy_texture_to_texture(
            target.main_texture().as_image_copy(),
            copy.texture.as_image_copy(),
            Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
        );
        &copy.default_view
    };
    let bind_group = ctx.render_device().create_bind_group(
        "water_bind_group",
        &layout,
        &BindGroupEntries::sequential((depth.view(), scene, &gpu.sampler, gpu.uniform.as_entire_binding())),
    );

    let color = match target.sampled_main_texture_view().filter(|_| multisampled) {
        Some(samples) => RenderPassColorAttachment {
            view: samples,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        },
        None => target.get_color_attachment(),
    };
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
    pass.set_bind_group(3, &bind_group, &[]);
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
    drop(pass);

    // Nothing else resolves the multisampled image this frame: resolve it here.
    let transparent_follows =
        transparent_phases.get(&extracted.retained_view_entity).is_some_and(|phase| !phase.items.is_empty());
    if multisampled && !transparent_follows {
        ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("water_resolve"),
            color_attachments: &[Some(target.get_color_attachment())],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octave_weights_sum_to_one() {
        for count in [1, 8, MAX_OCTAVES] {
            let sum: f32 = octaves(count, Vec2::new(3.0, 1.0)).iter().map(|o| o.weight).sum();
            assert!((sum - 1.0).abs() < 1e-5, "{count} octaves weigh {sum}");
        }
        assert!(octaves(0, Vec2::ZERO).is_empty());
    }

    #[test]
    fn wind_lines_up_the_long_waves() {
        let wind = Vec2::new(0.0, 10.0);
        let along = |octaves: &[Octave]| octaves[..4].iter().map(|o| o.direction.dot(wind.normalize()).abs()).sum::<f32>();
        let calm = octaves(MAX_OCTAVES, Vec2::ZERO);
        let windy = octaves(MAX_OCTAVES, wind);
        assert!(along(&windy) > along(&calm) + 0.5);
        assert!(windy.iter().all(|o| (o.direction.length() - 1.0).abs() < 1e-5));
    }

    #[test]
    fn phases_are_continuous_across_origins() {
        // Far from the world's origin, a point's phase measured from either of the two
        // snapped origins around it is the same, so waves never jump as the camera moves.
        let octave = octaves(MAX_OCTAVES, Vec2::new(4.0, -2.0))[5];
        let point = DVec2::new(1_000_123.25, -7_654_321.5);
        let seconds = 12_345.678;
        let first = (point / ORIGIN_SNAP).floor() * ORIGIN_SNAP;
        let shader = |origin: DVec2| {
            let offset = (point - origin).as_vec2();
            phase(&octave, origin, seconds) + octave.direction.dot(offset) * octave.frequency
        };
        let difference = (shader(first) - shader(first + DVec2::new(ORIGIN_SNAP, 0.0))).rem_euclid(std::f32::consts::TAU);
        assert!(difference.min(std::f32::consts::TAU - difference) < 1e-3, "{difference}");
    }
}
