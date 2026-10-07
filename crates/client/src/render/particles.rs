//! Particles: small quads facing the camera, showing a piece of a block texture or a soft
//! speck of colour, each following a path the vertex shader works out from where and when
//! it started. The CPU writes a particle once, when it is spawned, and never touches it
//! again; every live particle is drawn by one instanced draw, with nothing per particle on
//! the CPU per frame.
//!
//! Effects spawn particles through [`Particles`]. They live in a ring of [`CAPACITY`]
//! 64-byte records on the GPU, the oldest overwritten when it is full. Each is drawn one of
//! three ways (its [`ParticleBlend`]):
//!
//! - cut out, with the alpha-tested terrain (depth-tested and depth-writing like any block,
//!   so they need no sorting), fading at the end of their life through alpha to coverage
//!   under MSAA and a dither otherwise: pieces of broken blocks;
//! - adding their light to what is behind them: flames, embers and sparks, so overlapping
//!   tongues merge into one flame instead of a stack of cut-out teardrops;
//! - as a soft veil (premultiplied alpha): smoke.
//!
//! The last two are drawn in a pass of their own after the water and the volumetric air,
//! before Bevy's transparent pass, against the scene's depth without writing it: they fade
//! out where they come near anything solid, so smoke never shows a hard line where it
//! meets a wall. Each draw goes oldest first (the ring in age order), so overlapping veils
//! keep their order from frame to frame and never pop. Every draw covers the whole ring;
//! a particle drawn another way collapses to nothing in the vertex shader.
//!
//! How a particle moves is its [`Motion`], evaluated in `particles.wgsl`. New kinds of
//! motion are a new variant and a new case in the shader; nothing else changes. Breaking
//! throws pieces (`Ballistic`); fires and torches give off smoke, embers and flames that
//! `Rise` (crate::emitters).


use bevy::core_pipeline::core_3d::{AlphaMask3d, main_transparent_pass_3d};
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_phase::{CachedRenderPipelinePhaseItem, DrawFunctionId, PhaseItemExtraIndex};
use bevy::render::render_resource::{
    BlendComponent, BlendFactor, BlendOperation, BlendState, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDepthStencilAttachment, RenderPassDescriptor, StoreOp,
    binding_types::{texture_depth_2d, texture_depth_2d_multisampled},
};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::view::{RetainedViewEntity, ViewDepthTexture, ViewTarget};
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
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::asset::embedded_asset;
use bytemuck::{Pod, Zeroable};
use rustc_hash::FxHashMap;

use super::atmosphere::{Atmosphere, AtmosphereBuffer};
use super::LayerInfo;
use super::cloud_map::{self, CloudMap};
use super::terrain::{TerrainTextures, mesh_like};
use crate::textures::Rgba;
use voxmar_engine::block::{ParticleBlend, Tint};

/// Particles alive at once at most.
pub const CAPACITY: u32 = 8192;
const SHADER: &str = "embedded://voxmar/render/particles.wgsl";

pub struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "particles.wgsl");
        app.init_resource::<Particles>().add_systems(First, Particles::tick);
        let key = MainEntity::from(app.world_mut().spawn_empty().id());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .insert_resource(ParticleKey(key))
            .init_resource::<ParticleInbox>()
            .add_render_command::<AlphaMask3d, DrawParticleQuads>()
            .init_resource::<DrawFunctions<BlendedItem>>()
            .add_render_command::<BlendedItem, DrawBlended>()
            .add_systems(RenderStartup, init_gpu)
            .add_systems(ExtractSchedule, extract)
            .add_systems(
                Render,
                (
                    upload.in_set(RenderSystems::PrepareResources),
                    prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
                    queue.in_set(RenderSystems::Queue),
                ),
            )
            .add_systems(
                Core3d,
                blended_pass
                    .after(super::water::WaterPass)
                    .after(super::volumetric::VolumetricPass)
                    .before(main_transparent_pass_3d)
                    .in_set(Core3dSystems::MainPass),
            );
    }
}

/// How a particle moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Motion {
    /// Thrown with its velocity: falls with gravity, slowed by drag, then hops a few times,
    /// lower each time, on the floor it was given and slides to rest.
    Ballistic = 0,
    /// Carried up by its own warmth, as smoke, embers and flames are: moves with its
    /// velocity slowed by drag (gravity, negative, lifts it on), wandering across in slow
    /// loops that widen as it rises, fading in as it starts.
    Rise = 1,
}

/// What a particle shows.
#[derive(Clone, Copy, Debug)]
pub enum Look {
    /// A square piece of a block texture, `corner` and `span` in texture widths (0..1),
    /// multiplied by the particle's colour (a tinted block's biome colour).
    Texture { layer: u16, corner: Vec2, span: f32 },
    /// A soft round speck of the particle's colour.
    Speck,
}

/// A particle as an effect spawns it.
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub position: Vec3,
    /// Blocks per second.
    pub velocity: Vec3,
    pub motion: Motion,
    /// Blocks per second squared, downwards.
    pub gravity: f32,
    /// How quickly its speed decays, per second.
    pub drag: f32,
    /// The height its centre comes to rest at.
    pub floor: f32,
    /// Width in blocks.
    pub size: f32,
    /// Its width at the end of its life, as a multiple of `size` (0..8): smoke swells, a
    /// flame shrinks away.
    pub grow: f32,
    /// How far it wanders across while it rises, in blocks (0..0.63; `Motion::Rise`).
    pub wander: f32,
    /// Seconds; it fades out over the last part.
    pub life: f32,
    pub look: Look,
    /// sRGB colour that multiplies a texture or colours a speck; alpha is opacity.
    pub color: [u8; 4],
    /// Shines by its own light instead of being lit (fireflies, motes).
    pub glow: bool,
    /// Glowing, it cools as it ages, dimming from yellow through orange to a dull red
    /// (embers, sparks).
    pub cools: bool,
    /// Sky and block light where it is, 0..=15, which light it like the terrain there.
    pub light: [u8; 2],
    /// How it is drawn over what is behind it.
    pub blend: ParticleBlend,
    /// How much of what lies behind it a particle that adds light hides, 0..1.
    pub hides: f32,
}

impl Default for Particle {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            motion: Motion::Ballistic,
            gravity: 0.0,
            drag: 0.0,
            floor: f32::MIN,
            size: 0.1,
            grow: 1.0,
            wander: 0.0,
            life: 1.0,
            look: Look::Speck,
            color: [255; 4],
            glow: false,
            cools: false,
            light: [15, 0],
            blend: ParticleBlend::Cutout,
            hides: 0.0,
        }
    }
}

/// A particle as the shader reads it (`Particle` in particles.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
struct GpuParticle {
    origin: [f32; 3],
    /// `globals.time` when it was spawned.
    born: f32,
    velocity: [f32; 3],
    floor: f32,
    size: f32,
    life: f32,
    gravity: f32,
    drag: f32,
    /// layer:12 | grow:6 (eighths) | wander:6 (hundredths of a block) | block light:4 |
    /// sky light:4.
    surface: u32,
    /// corner u:8 | corner v:8 | span:8 (in 256ths of the texture) | motion:2 | blend:2 |
    /// flags:4
    sprite: u32,
    /// sRGB colour and opacity, 8 bits each.
    color: u32,
    /// For motions that wander (the bits from 12 up), and in the low 8 how much of what
    /// lies behind it the particle hides, in 255ths.
    seed: u32,
}

const FLAG_TEXTURED: u32 = 1;
const FLAG_GLOW: u32 = 2;
const FLAG_COOLS: u32 = 4;

/// A blend as the shader reads it (`BLEND_*` in particles.wgsl).
fn blend_bits(blend: ParticleBlend) -> u32 {
    match blend {
        ParticleBlend::Cutout => 0,
        ParticleBlend::Add => 1,
        ParticleBlend::Veil => 2,
    }
}

impl GpuParticle {
    fn new(particle: &Particle, born: f32, seed: u32) -> Self {
        let [sky, block] = particle.light.map(|l| l.min(15) as u32);
        let (surface, sprite, flags) = match particle.look {
            Look::Texture { layer, corner, span } => {
                let surface = layer as u32 & 0xFFF;
                let unit = |x: f32| (x * 256.0).round().clamp(0.0, 255.0) as u32;
                (surface, unit(corner.x) | unit(corner.y) << 8 | unit(span) << 16, FLAG_TEXTURED)
            }
            Look::Speck => (0, 0, 0),
        };
        let flags = flags | if particle.glow { FLAG_GLOW } else { 0 } | if particle.cools { FLAG_COOLS } else { 0 };
        let six_bits = |x: f32| (x.round().clamp(0.0, 63.0)) as u32;
        let shape = six_bits(particle.grow * 8.0) << 12 | six_bits(particle.wander * 100.0) << 18;
        Self {
            origin: particle.position.to_array(),
            born,
            velocity: particle.velocity.to_array(),
            floor: particle.floor,
            size: particle.size,
            life: particle.life,
            gravity: particle.gravity,
            drag: particle.drag,
            surface: surface | shape | block << 24 | sky << 28,
            sprite: sprite | (particle.motion as u32) << 24 | blend_bits(particle.blend) << 26 | flags << 28,
            color: u32::from_le_bytes(particle.color),
            seed: seed & !0xFF | (particle.hides.clamp(0.0, 1.0) * 255.0).round() as u32,
        }
    }
}

/// The main world's handle on particles: effects spawn through it.
#[derive(Resource, Default)]
pub struct Particles {
    /// Spawned since the last frame was extracted.
    spawned: Vec<GpuParticle>,
    /// `globals.time` this frame, which particles spawned now are born at.
    now: f32,
    /// Seconds since startup, and when the last particle spawned so far dies.
    elapsed: f64,
    alive_until: f64,
    count: u32,
    /// How many particles effects should spawn, as a share of their full amount: the
    /// player's particle setting times the shader pack's.
    pub amount: f32,
}

impl Particles {
    pub fn spawn(&mut self, particle: &Particle) {
        self.count = self.count.wrapping_add(1);
        let seed = self.count.wrapping_mul(0x9e37_79b9);
        self.spawned.push(GpuParticle::new(particle, self.now, seed));
        self.alive_until = self.alive_until.max(self.elapsed + particle.life as f64);
    }

    fn tick(time: Res<Time>, mut particles: ResMut<Particles>) {
        particles.now = time.elapsed_secs_wrapped();
        particles.elapsed = time.elapsed_secs_f64();
    }
}

/// Square pieces of block textures that are mostly opaque, so a piece of a flower or a
/// leaf shows its petals and leaves rather than empty texels, and the tint of each layer's
/// overlay (the shader draws it over a piece as the terrain does).
#[derive(Resource)]
pub struct TexturePieces {
    /// Per texture layer, one bit per piece of a 4×4 grid (bit `y * 4 + x`).
    solid: Vec<u16>,
    overlays: Vec<Tint>,
}

impl TexturePieces {
    pub fn new(textures: &[Rgba], layers: &[LayerInfo]) -> Self {
        let solid = textures
            .iter()
            .map(|texture| {
                let side = texture.width.min(texture.height).max(4);
                let piece = side / 4;
                let mut mask = 0u16;
                for cell in 0..16 {
                    let (cx, cy) = (cell % 4, cell / 4);
                    let mut opaque = 0;
                    for y in cy * piece..(cy + 1) * piece {
                        for x in cx * piece..(cx + 1) * piece {
                            let i = ((y * texture.width + x) * 4 + 3) as usize;
                            opaque += (texture.pixels.get(i).copied().unwrap_or(0) >= 128) as u32;
                        }
                    }
                    if opaque * 2 >= piece * piece {
                        mask |= 1 << cell;
                    }
                }
                mask
            })
            .collect();
        let overlays = layers.iter().map(|layer| layer.overlay.map_or(Tint::None, |(_, tint)| tint)).collect();
        Self { solid, overlays }
    }

    /// The tint of the overlay drawn over `layer`, which a piece's colour is then for.
    pub fn overlay_tint(&self, layer: u16) -> Tint {
        self.overlays.get(layer as usize).copied().unwrap_or(Tint::None)
    }

    /// The corner of a quarter-width piece of `layer`, picked by `random` (0..1) among its
    /// mostly opaque pieces, or anywhere if it has none.
    pub fn corner(&self, layer: u16, random: f32) -> Vec2 {
        let mask = self.solid.get(layer as usize).copied().unwrap_or(0);
        let mask = if mask == 0 { u16::MAX } else { mask };
        let pick = ((random * mask.count_ones() as f32) as u32).min(mask.count_ones() - 1);
        let mut bits = mask;
        for _ in 0..pick {
            bits &= bits - 1;
        }
        let cell = bits.trailing_zeros();
        Vec2::new((cell % 4) as f32, (cell / 4) as f32) * 0.25
    }
}

#[derive(Resource)]
struct ParticleKey(MainEntity);

/// What extraction hands the render world.
#[derive(Resource, Default)]
struct ParticleInbox {
    spawned: Vec<GpuParticle>,
    /// Whether any particle may still be alive.
    alive: bool,
}

fn extract(mut main: ResMut<MainWorld>, mut inbox: ResMut<ParticleInbox>) {
    let Some(mut particles) = main.get_resource_mut::<Particles>() else { return };
    inbox.alive = particles.elapsed < particles.alive_until;
    // Moved across, so both lists keep what they have grown to.
    inbox.spawned.append(&mut particles.spawned);
}

/// The ring of particles and the pipeline that draws them.
#[derive(Resource)]
struct ParticleGpu {
    buffer: Buffer,
    /// Where the next particle goes.
    head: u32,
    /// Slots ever written, which the draw covers.
    filled: u32,
    layout: BindGroupLayoutDescriptor,
    bind_group: Option<BindGroup>,
    shader: Handle<Shader>,
    vertex_layout: MeshVertexBufferLayout,
    pipelines: FxHashMap<MeshPipelineKey, CachedRenderPipelineId>,
    /// The blended pass's group 3, the scene's depth, without and with MSAA.
    depth_layouts: [BindGroupLayoutDescriptor; 2],
    /// Per view key, the pipelines that add light and that lay a veil.
    blended_pipelines: FxHashMap<MeshPipelineKey, [CachedRenderPipelineId; 2]>,
    /// The blended pipelines of each camera, for the blended pass.
    blended: FxHashMap<RetainedViewEntity, [CachedRenderPipelineId; 2]>,
    /// Render entity standing for the particles in the phases.
    item: Entity,
}

fn init_gpu(mut commands: Commands, device: Res<RenderDevice>, asset_server: Res<AssetServer>) {
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("particles"),
        size: CAPACITY as u64 * size_of::<GpuParticle>() as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut layout = BindGroupLayoutDescriptor::new(
        "particle_layout",
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
    // The particle pipeline is built as the terrain's (`mesh_like`), with its shader defs,
    // so it reads the cloud maps and noise too.
    layout.entries.extend(cloud_map::layout_entries(ShaderStages::VERTEX_FRAGMENT));
    // Particles have no vertex buffer: the vertex and instance indices say everything.
    let vertex_layout = MeshVertexBufferLayout::new(
        Vec::new(),
        VertexBufferLayout { array_stride: 0, step_mode: VertexStepMode::Vertex, attributes: Vec::new() },
    );
    let depth_layout = |depth| BindGroupLayoutDescriptor::new("particle_depth_layout", &BindGroupLayoutEntries::single(ShaderStages::FRAGMENT, depth));
    let depth_layouts = [depth_layout(texture_depth_2d()), depth_layout(texture_depth_2d_multisampled())];
    let item = commands.spawn_empty().id();
    commands.insert_resource(ParticleGpu {
        buffer,
        head: 0,
        filled: 0,
        layout,
        bind_group: None,
        shader: asset_server.load(SHADER),
        vertex_layout,
        pipelines: FxHashMap::default(),
        depth_layouts,
        blended_pipelines: FxHashMap::default(),
        blended: FxHashMap::default(),
        item,
    });
}

/// Writes the particles spawned this frame into the ring.
fn upload(mut gpu: ResMut<ParticleGpu>, mut inbox: ResMut<ParticleInbox>, queue: Res<RenderQueue>) {
    let mut spawned = inbox.spawned.as_slice();
    // More than the ring holds: only the newest matter.
    if spawned.len() > CAPACITY as usize {
        spawned = &spawned[spawned.len() - CAPACITY as usize..];
    }
    while !spawned.is_empty() {
        let room = (CAPACITY - gpu.head) as usize;
        let (now, rest) = spawned.split_at(room.min(spawned.len()));
        let offset = gpu.head as u64 * size_of::<GpuParticle>() as u64;
        queue.write_buffer(&gpu.buffer, offset, bytemuck::cast_slice(now));
        gpu.head = (gpu.head + now.len() as u32) % CAPACITY;
        gpu.filled = (gpu.filled + now.len() as u32).min(CAPACITY);
        spawned = rest;
    }
    inbox.spawned.clear();
}

fn prepare_bind_group(
    mut gpu: ResMut<ParticleGpu>,
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
        "particle_bind_group",
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

impl ParticleGpu {
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
        let mut descriptor =
            mesh_like(mesh_pipeline, layouts, key, &self.vertex_layout, &self.layout, &self.shader, "particles".into())?;
        descriptor.vertex.buffers.clear();
        descriptor.primitive.cull_mode = None;
        // Under MSAA, cut-outs and fading cover samples by their alpha.
        let alpha_to_coverage = key.msaa_samples() > 1;
        descriptor.multisample.alpha_to_coverage_enabled = alpha_to_coverage;
        if alpha_to_coverage && let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("ALPHA_TO_COVERAGE".into());
        }
        let id = cache.queue_render_pipeline(descriptor);
        self.pipelines.insert(view_key, id);
        Some(id)
    }

    /// The blended pipelines for a view, adding light and laying a veil: the mesh pipeline
    /// with our shader, testing depth without writing it, reading the scene's depth.
    fn blended_pipelines(
        &mut self,
        view_key: MeshPipelineKey,
        mesh_pipeline: &MeshPipeline,
        layouts: &mut MeshVertexBufferLayouts,
        cache: &PipelineCache,
    ) -> Option<[CachedRenderPipelineId; 2]> {
        if let Some(&ids) = self.blended_pipelines.get(&view_key) {
            return Some(ids);
        }
        let multisampled = view_key.msaa_samples() > 1;
        let mut make = |name: &str, def: &str, blend: BlendState| {
            let mut descriptor =
                mesh_like(mesh_pipeline, layouts, view_key, &self.vertex_layout, &self.layout, &self.shader, format!("particles_{name}"))?;
            descriptor.vertex.buffers.clear();
            descriptor.primitive.cull_mode = None;
            descriptor.layout.push(self.depth_layouts[multisampled as usize].clone());
            if let Some(depth) = descriptor.depth_stencil.as_mut() {
                depth.depth_write_enabled = Some(false);
            }
            let mut defs = vec!["BLENDED".into(), def.into()];
            if multisampled {
                defs.push("DEPTH_MULTISAMPLED".into());
            }
            descriptor.vertex.shader_defs.extend(defs.clone());
            let fragment = descriptor.fragment.as_mut()?;
            fragment.shader_defs.extend(defs);
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(blend);
            }
            Some(cache.queue_render_pipeline(descriptor))
        };
        // Light added over what is behind, which it also partly hides (by the alpha the
        // shader gives, `HIDES`): a flame against the bright day sky stays orange instead of
        // adding up with the sky's blue into peach.
        let add = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add };
        let keep = BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        let ids = [
            make("add", "BLEND_ADD", BlendState { color: add, alpha: keep })?,
            make("veil", "BLEND_VEIL", BlendState::PREMULTIPLIED_ALPHA_BLENDING)?,
        ];
        self.blended_pipelines.insert(view_key, ids);
        Some(ids)
    }
}

/// Puts the particles into every camera's alpha-tested phase while any may be alive.
#[allow(clippy::too_many_arguments)]
fn queue(
    mut gpu: ResMut<ParticleGpu>,
    inbox: Res<ParticleInbox>,
    key: Res<ParticleKey>,
    mesh_pipeline: Res<MeshPipeline>,
    mut layouts: ResMut<MeshVertexBufferLayouts>,
    cache: Res<PipelineCache>,
    view_keys: Res<ViewKeyCache>,
    mut phases: ResMut<ViewBinnedRenderPhases<AlphaMask3d>>,
    draw_functions: Res<DrawFunctions<AlphaMask3d>>,
    views: Query<&ExtractedView, (Without<LightEntity>, With<Camera3d>)>,
) {
    let draw_function = draw_functions.read().get_id::<DrawParticleQuads>().expect("particle draw is registered");
    let drawing = inbox.alive && gpu.filled > 0 && gpu.bind_group.is_some();
    for view in &views {
        let retained = &view.retained_view_entity;
        let (Some(phase), Some(&view_key)) = (phases.get_mut(retained), view_keys.get(retained)) else { continue };
        phase.remove(key.0);
        // Asked for while nothing is drawn too, so it is compiled as the world loads: on
        // Metal, Bevy compiles on the render thread, which held the frame of the first
        // block broken.
        let Some(pipeline) = gpu.pipeline(view_key, &mesh_pipeline, &mut layouts, &cache) else { continue };
        // The blended pass's too, for the same reason.
        if let Some(ids) = gpu.blended_pipelines(view_key, &mesh_pipeline, &mut layouts, &cache) {
            gpu.blended.insert(retained.clone(), ids);
        }
        if !drawing {
            continue;
        }
        let batch = OpaqueNoLightmap3dBatchSetKey { pipeline, draw_function, material_bind_group_index: None, slabs: default() };
        let bin = OpaqueNoLightmap3dBinKey { asset_id: AssetId::<Mesh>::default().untyped() };
        phase.add(batch, bin, (gpu.item, key.0), InputUniformIndex::default(), BinnedRenderPhaseType::NonMesh);
    }
}

type DrawParticleQuads = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawParticles);

/// Draws every slot of the ring; the dead and unborn collapse to nothing in the shader.
struct DrawParticles;

impl<P: PhaseItem> RenderCommand<P> for DrawParticles {
    type Param = SRes<ParticleGpu>;
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
        // Oldest first: from the slot after the newest to the end, then from the start.
        let head = gpu.head.min(gpu.filled);
        if head < gpu.filled {
            render_pass.draw(0..6, head..gpu.filled);
        }
        if head > 0 {
            render_pass.draw(0..6, 0..head);
        }
        RenderCommandResult::Success
    }
}

/// Stands for the particles in the blended pass, to run their draw commands.
struct BlendedItem {
    pipeline: CachedRenderPipelineId,
    entity: (Entity, MainEntity),
    draw_function: DrawFunctionId,
    batch_range: std::ops::Range<u32>,
    extra_index: PhaseItemExtraIndex,
}

impl PhaseItem for BlendedItem {
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

impl CachedRenderPipelinePhaseItem for BlendedItem {
    fn cached_pipeline(&self) -> CachedRenderPipelineId {
        self.pipeline
    }
}

/// The blended particles' draw commands; `blended_pass` binds group 3, the depth, itself.
type DrawBlended = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawParticles);

/// What the blended pass needs of a camera.
type BlendedView =
    (&'static ExtractedCamera, &'static ExtractedView, &'static ViewTarget, &'static ViewDepthTexture, &'static Msaa);

/// Draws the particles that add light or lay a veil, into the scene as it is after the
/// water and the air (the multisampled image under MSAA, resolved later with the rest),
/// testing against its depth through a read-only attachment and reading the same depth to
/// fade where they meet anything solid. Veils first, then the light added over them.
fn blended_pass(
    world: &World,
    view: ViewQuery<BlendedView>,
    gpu: Option<Res<ParticleGpu>>,
    inbox: Res<ParticleInbox>,
    key: Res<ParticleKey>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let view_entity = view.entity();
    let (camera, extracted, target, depth, msaa) = view.into_inner();
    let Some(gpu) = gpu else { return };
    if !inbox.alive || gpu.filled == 0 || gpu.bind_group.is_none() {
        return;
    }
    let Some(&[add, veil]) = gpu.blended.get(&extracted.retained_view_entity) else { return };
    let multisampled = *msaa != Msaa::Off;
    let color = match target.sampled_main_texture_view().filter(|_| multisampled) {
        Some(samples) => samples,
        None => target.main_texture_view(),
    };
    let depth_group = ctx.render_device().create_bind_group(
        "particle_depth_bind_group",
        &pipeline_cache.get_bind_group_layout(&gpu.depth_layouts[multisampled as usize]),
        &BindGroupEntries::single(depth.view()),
    );
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("blended_particles"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        })],
        // Read-only, so the same depth can be sampled while it is tested against.
        depth_stencil_attachment: Some(RenderPassDepthStencilAttachment { view: depth.view(), depth_ops: None, stencil_ops: None }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Some(viewport) = camera.viewport.as_ref() {
        pass.set_camera_viewport(viewport);
    }
    pass.set_bind_group(3, &depth_group, &[]);
    let functions = world.resource::<DrawFunctions<BlendedItem>>();
    let mut functions = functions.write();
    functions.prepare(world);
    let Some(id) = functions.get_id::<DrawBlended>() else { return };
    for pipeline in [veil, add] {
        let item = BlendedItem { pipeline, entity: (gpu.item, key.0), draw_function: id, batch_range: 0..1, extra_index: PhaseItemExtraIndex::None };
        if let Some(draw) = functions.get_mut(id)
            && let Err(e) = draw.draw(world, &mut pass, view_entity, &item)
        {
            error!("blended particles: {e:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particles_pack_like_the_shader_reads_them() {
        assert_eq!(size_of::<GpuParticle>(), 64);
        let particle = Particle {
            look: Look::Texture { layer: 300, corner: Vec2::new(0.25, 0.75), span: 0.25 },
            light: [14, 3],
            glow: true,
            cools: true,
            grow: 3.5,
            wander: 0.25,
            motion: Motion::Rise,
            color: [10, 20, 30, 40],
            ..default()
        };
        let gpu = GpuParticle::new(&particle, 5.0, 0);
        assert_eq!(gpu.surface & 0xFFF, 300);
        assert_eq!((gpu.surface >> 12 & 63, gpu.surface >> 18 & 63), (28, 25));
        assert_eq!((gpu.surface >> 24 & 15, gpu.surface >> 28), (3, 14));
        assert_eq!((gpu.sprite & 255, gpu.sprite >> 8 & 255, gpu.sprite >> 16 & 255), (64, 192, 64));
        assert_eq!(gpu.sprite >> 24 & 3, Motion::Rise as u32);
        assert_eq!(gpu.sprite >> 26 & 3, 0);
        let added = GpuParticle::new(&Particle { blend: ParticleBlend::Add, ..particle }, 5.0, 0);
        assert_eq!((added.sprite >> 24 & 3, added.sprite >> 26 & 3), (Motion::Rise as u32, 1));
        let veiled = GpuParticle::new(&Particle { blend: ParticleBlend::Veil, ..particle }, 5.0, 0);
        assert_eq!(veiled.sprite >> 26 & 3, 2);
        assert_eq!(gpu.sprite >> 28, FLAG_TEXTURED | FLAG_GLOW | FLAG_COOLS);
        // An unchanged size packs as eight eighths.
        assert_eq!(GpuParticle::new(&Particle::default(), 0.0, 0).surface >> 12 & 63, 8);
        assert_eq!(gpu.color.to_le_bytes(), [10, 20, 30, 40]);
    }

    #[test]
    fn pieces_come_from_opaque_parts_of_a_texture() {
        // Opaque only in the bottom-right quarter (pieces x, y in 2..4).
        let size = 16;
        let mut pixels = vec![0u8; size * size * 4];
        for y in 8..16 {
            for x in 8..16 {
                pixels[(y * size + x) * 4 + 3] = 255;
            }
        }
        let pieces = TexturePieces::new(&[Rgba { width: 16, height: 16, pixels }], &[]);
        for i in 0..20 {
            let corner = pieces.corner(0, i as f32 / 20.0);
            assert!(corner.x >= 0.5 && corner.y >= 0.5, "{corner}");
        }
        // A fully transparent texture still gives pieces.
        let empty = TexturePieces::new(&[Rgba { width: 16, height: 16, pixels: vec![0; 1024] }], &[]);
        assert!(empty.corner(0, 0.99).max_element() <= 0.75);
    }
}
