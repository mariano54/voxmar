//! The blur behind menus: the world (or the title screen's demo world) goes soft while a
//! menu, or more lightly the inventory, is open, so the panel in front reads clearly and
//! the game still shows through.
//!
//! Four full-screen Gaussian passes on the main camera, across and down twice, after the
//! display transform. They run only while something is blurred: closing the menu takes
//! the blur off the camera, so playing costs nothing. (Bevy's `FullscreenMaterial` would
//! be shorter, but it keeps one pipeline per camera, so two materials on one camera, the
//! display transform and this, would draw with each other's pipeline.) The blur eases in and out over a moment,
//! and its width follows the screen's height, so it looks the same at any resolution.

use bevy::asset::{AssetServer, embedded_asset};
use bevy::core_pipeline::fullscreen_material::fullscreen_material_system;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId, ColorTargetState,
    ColorWrites, DynamicUniformBuffer, FragmentState, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    ShaderType, TextureFormat, TextureSampleType,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use crate::menu::Screen;
use crate::player::UiState;
use crate::render::DisplayTransform;

/// Each pass's standard deviation at full strength, as a share of the screen's height.
/// Two passes along each axis add up to √2 times that.
const SIGMA: f32 = 0.0085;
/// The inventory blurs less than a menu: the world is still the point.
const INVENTORY: f32 = 0.45;
/// Seconds to blur fully, or to clear.
const EASE: f32 = 0.18;
const SHADER: &str = "embedded://voxmar/ui/blur.wgsl";

pub struct BlurPlugin;

impl Plugin for BlurPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "blur.wgsl");
        app.add_plugins(ExtractComponentPlugin::<MenuBlur>::default())
            .init_resource::<Strength>()
            .add_systems(PostUpdate, blur);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare.in_set(RenderSystems::Prepare))
            .add_systems(
                Core3d,
                draw.in_set(Core3dSystems::PostProcess).after(fullscreen_material_system::<DisplayTransform>).before(tonemapping),
            );
    }
}

/// How blurred the backdrop is now, 0..1.
#[derive(Resource, Default)]
struct Strength(f32);

/// On a camera while its picture is blurred: each pass's standard deviation in pixels.
#[derive(Component, ExtractComponent, Clone, Copy)]
struct MenuBlur {
    sigma: f32,
}

/// Eases the blur towards what the screen wants, and keeps it on the camera while
/// anything is blurred.
fn blur(
    mut commands: Commands,
    time: Res<Time<Real>>,
    screen: Res<State<Screen>>,
    ui: Res<UiState>,
    mut strength: ResMut<Strength>,
    camera: Query<(Entity, &Camera, Has<MenuBlur>), With<Camera3d>>,
) {
    let wanted = match screen.get() {
        Screen::Worlds | Screen::CreateWorld | Screen::Join | Screen::Settings => 1.0,
        Screen::InGame if ui.menu_open => 1.0,
        Screen::InGame if ui.inventory_open => INVENTORY,
        _ => 0.0,
    };
    let step = time.delta_secs() / EASE;
    strength.0 = if strength.0 < wanted { (strength.0 + step).min(wanted) } else { (strength.0 - step).max(wanted) };
    let Ok((entity, camera, blurred)) = camera.single() else { return };
    if strength.0 <= 0.0 {
        if blurred {
            commands.entity(entity).remove::<MenuBlur>();
        }
        return;
    }
    let height = camera.physical_target_size().map_or(1080.0, |size| size.y as f32);
    // Ease out: most of the blur arrives early.
    let eased = 1.0 - (1.0 - strength.0).powi(2);
    commands.entity(entity).insert(MenuBlur { sigma: SIGMA * height * eased });
}

/// What one pass reads: its axis and width.
#[derive(ShaderType, Clone, Copy)]
struct PassUniform {
    direction: Vec2,
    sigma: f32,
}

#[derive(Resource)]
struct BlurPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    shader: Handle<Shader>,
    /// One pipeline per format of the image blurred.
    pipelines: HashMap<TextureFormat, CachedRenderPipelineId>,
    /// Both axes' uniforms for every blurred view, rewritten each frame.
    uniforms: DynamicUniformBuffer<PassUniform>,
}

/// A blurred view's pipeline and the offsets of its two axes' uniforms.
#[derive(Component)]
struct PreparedBlur {
    pipeline: CachedRenderPipelineId,
    offsets: [u32; 2],
}

fn init_pipeline(mut commands: Commands, device: Res<RenderDevice>, assets: Res<AssetServer>) {
    let layout = BindGroupLayoutDescriptor::new(
        "menu_blur_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<PassUniform>(true),
            ),
        ),
    );
    commands.insert_resource(BlurPipeline {
        layout,
        sampler: device.create_sampler(&SamplerDescriptor::default()),
        shader: assets.load(SHADER),
        pipelines: HashMap::default(),
        uniforms: DynamicUniformBuffer::default(),
    });
}

fn prepare(
    mut commands: Commands,
    mut pipeline: ResMut<BlurPipeline>,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(Entity, &ExtractedView, Option<&MenuBlur>)>,
) {
    pipeline.uniforms.clear();
    for (entity, view, blur) in &views {
        let Some(blur) = blur else {
            commands.entity(entity).remove::<PreparedBlur>();
            continue;
        };
        let format = view.target_format;
        let BlurPipeline { layout, shader, pipelines, .. } = &mut *pipeline;
        let id = *pipelines.entry(format).or_insert_with(|| {
            cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("menu_blur_pipeline".into()),
                layout: vec![layout.clone()],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                    ..default()
                }),
                ..default()
            })
        });
        let offsets = [Vec2::X, Vec2::Y].map(|direction| pipeline.uniforms.push(&PassUniform { direction, sigma: blur.sigma }));
        commands.entity(entity).insert(PreparedBlur { pipeline: id, offsets });
    }
    pipeline.uniforms.write_buffer(&device, &queue);
}

/// Four passes, ping-ponging between the view's two post-processing images.
fn draw(
    view: ViewQuery<(&ViewTarget, &PreparedBlur)>,
    pipeline: Res<BlurPipeline>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (target, prepared) = view.into_inner();
    let (Some(render_pipeline), Some(uniforms)) = (cache.get_render_pipeline(prepared.pipeline), pipeline.uniforms.binding()) else {
        return;
    };
    let layout = cache.get_bind_group_layout(&pipeline.layout);
    for pass in 0..4 {
        let post_process = target.post_process_write();
        let bind_group = device.create_bind_group(
            "menu_blur_bind_group",
            &layout,
            &BindGroupEntries::sequential((post_process.source, &pipeline.sampler, uniforms.clone())),
        );
        let descriptor = RenderPassDescriptor {
            label: Some("menu_blur_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post_process.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        };
        let mut render_pass = ctx.command_encoder().begin_render_pass(&descriptor);
        render_pass.set_pipeline(render_pipeline);
        render_pass.set_bind_group(0, &bind_group, &[prepared.offsets[pass % 2]]);
        render_pass.draw(0..3, 0..1);
    }
}
