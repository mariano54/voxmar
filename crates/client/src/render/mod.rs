//! GPU side of the voxel world: terrain material, texture arrays, camera, sun and sky.
//!
//! Time of day drives a real directional light (with cascaded shadow maps), the camera's
//! ambient light and distance fog. Shaders read those from the view bind group, so the
//! day/night cycle never modifies material assets (which would respecialise every chunk).

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::Hdr;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::core_pipeline::Core3dSystems;
use bevy::core_pipeline::fullscreen_material::{FullscreenMaterial, FullscreenMaterialPlugin};
use bevy::core_pipeline::tonemapping::{Tonemapping, tonemapping};
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::BoxedSystem;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap, ShadowFilteringMethod};
use bevy::math::DVec2;
use bevy::mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{DistanceFog, FogFalloff, MaterialPipeline, MaterialPipelineKey};
use bevy::post_process::bloom::{Bloom, bloom};
use bevy::render::extract_component::ExtractComponent;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, TextureDataOrder,
    TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension, VertexFormat,
};
use bevy::shader::{ShaderRef, load_shader_library};
use voxmar_engine::mesh::MeshBuffers;
use voxmar_protocol::{TICKS_PER_DAY, Weather};

use crate::textures::Rgba;
use crate::weather::{ClientWeather, DRIFT_WRAP};

pub mod lod;
pub mod precipitation;

pub const ATTRIBUTE_VOXEL: MeshVertexAttribute = MeshVertexAttribute::new("Voxel", 0x766f_786d, VertexFormat::Uint32x2);

const VOXEL_SHADER: &str = "embedded://voxmar/render/voxel.wgsl";

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "lighting.wgsl");
        embedded_asset!(app, "voxel.wgsl");
        embedded_asset!(app, "sky.wgsl");
        embedded_asset!(app, "display.wgsl");
        app.add_plugins((
            MaterialPlugin::<VoxelMaterial>::default(),
            MaterialPlugin::<SkyMaterial>::default(),
            FullscreenMaterialPlugin::<DisplayTransform>::default(),
            lod::LodPlugin,
            precipitation::PrecipitationPlugin,
        ))
            .insert_resource(DirectionalLightShadowMap { size: 2048 })
            .init_resource::<DayCycle>()
            .add_systems(PostUpdate, update_environment.before(TransformSystems::Propagate));
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
#[bind_group_data(VoxelMaterialKey)]
pub struct VoxelMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    pub textures: Handle<Image>,
    #[texture(2, dimension = "2d_array")]
    #[sampler(3)]
    pub colormaps: Handle<Image>,
    pub pass: TerrainPass,
}

/// Terrain is drawn in four passes; each mesh pass of a chunk uses the matching material.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TerrainPass {
    Opaque,
    Cutout,
    /// Cut-out, double-sided faces between two leaves, which cast no shadow.
    Inner,
    Translucent,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoxelMaterialKey {
    pass: TerrainPass,
}

impl From<&VoxelMaterial> for VoxelMaterialKey {
    fn from(material: &VoxelMaterial) -> Self {
        Self { pass: material.pass }
    }
}

impl Material for VoxelMaterial {
    fn vertex_shader() -> ShaderRef {
        VOXEL_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        VOXEL_SHADER.into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        VOXEL_SHADER.into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        VOXEL_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        match self.pass {
            TerrainPass::Opaque => AlphaMode::Opaque,
            // Cut-out faces need the alpha test in the depth and shadow passes too.
            TerrainPass::Cutout | TerrainPass::Inner => AlphaMode::Mask(0.5),
            TerrainPass::Translucent => AlphaMode::Blend,
        }
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[ATTRIBUTE_VOXEL.at_shader_location(0)])?];
        // Faces are one-sided (plants emit both windings); the prepass and shadow pipelines
        // default to no culling.
        descriptor.primitive.cull_mode = Some(bevy::render::render_resource::Face::Back);
        let defs: &[&str] = match key.bind_group_data.pass {
            TerrainPass::Opaque => &[],
            TerrainPass::Cutout => &["CUTOUT"],
            TerrainPass::Inner => {
                // One face serves the two leaves on either side of it.
                descriptor.primitive.cull_mode = None;
                &["CUTOUT", "INNER"]
            }
            TerrainPass::Translucent => {
                // Water is visible from below too.
                descriptor.primitive.cull_mode = None;
                &["TRANSLUCENT"]
            }
        };
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.extend(defs.iter().map(|&def| def.into()));
        }
        Ok(())
    }
}

/// Maps the HDR image to the screen in place of a tonemapper (see `display.wgsl`): colours
/// up to 0.8 pass through unchanged, so blocks keep their texture colours in daylight as in
/// Minecraft, and brighter ones (the sun, glints, light sources) roll off to white.
#[derive(Component, ExtractComponent, ShaderType, Clone, Copy)]
pub struct DisplayTransform {
    /// Scales the image before the curve.
    pub exposure: f32,
}

impl Default for DisplayTransform {
    fn default() -> Self {
        Self { exposure: 1.0 }
    }
}

impl FullscreenMaterial for DisplayTransform {
    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/render/display.wgsl".into()
    }

    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        system.in_set(Core3dSystems::PostProcess).after(bloom).before(tonemapping)
    }
}

#[derive(ShaderType, Clone, Copy, Default, Debug)]
pub struct SkyParams {
    pub zenith: Vec4,
    pub horizon: Vec4,
    pub sun: Vec4,
    /// x: seconds of game time, y: low cloud cover, z: how heaped it is, w: cirrus.
    pub clouds: Vec4,
    /// x: fraction of the day, which turns the night sky with the sun; y: days elapsed
    /// (wrapped every 1000 days), for the slow drift of the planets.
    pub stars: Vec4,
    /// xy: the cloud layer's offset over the camera in cloud units, z: precipitation,
    /// w: lightning flash.
    pub weather: Vec4,
    /// xy: wind direction.
    pub wind: Vec4,
}

/// Blocks per unit of the sky shader's cloud noise. The cloud layer's offset wraps every
/// 4096 units, which the wind drift's wrapping matches, so wrapping never shows.
const CLOUD_UNIT: f64 = DRIFT_WRAP as f64 / 4096.0;

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub params: SkyParams,
}

impl Material for SkyMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/render/sky.wgsl".into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// Handles to the shared terrain materials, indexed by `TerrainPass`.
#[derive(Resource)]
pub struct VoxelMaterials {
    pub passes: [Handle<VoxelMaterial>; 4],
    pub sky: Handle<SkyMaterial>,
}

/// Server time, advanced locally between updates.
#[derive(Resource, Default)]
pub struct DayCycle {
    pub ticks: f64,
    /// Fog end distance in blocks: the edge of the distant terrain.
    pub view_distance: f32,
    pub underwater: bool,
}

#[derive(Component)]
pub struct SkyDome;

#[derive(Component)]
pub struct Sun;

/// Creates the texture arrays, materials, camera, sun and sky dome.
pub fn setup(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    meshes: &mut Assets<Mesh>,
    voxel_materials: &mut Assets<VoxelMaterial>,
    sky_materials: &mut Assets<SkyMaterial>,
    block_textures: &[Rgba],
    colormaps: &[Rgba],
) {
    let textures = images.add(array_image(block_textures, true, ImageAddressMode::Repeat));
    let colormaps = images.add(array_image(colormaps, false, ImageAddressMode::ClampToEdge));
    let material = |pass| VoxelMaterial { textures: textures.clone(), colormaps: colormaps.clone(), pass };
    let sky = sky_materials.add(SkyMaterial { params: SkyParams::default() });
    commands.insert_resource(VoxelMaterials {
        passes: [TerrainPass::Opaque, TerrainPass::Cutout, TerrainPass::Inner, TerrainPass::Translucent]
            .map(|pass| voxel_materials.add(material(pass))),
        sky: sky.clone(),
    });
    commands.spawn((
        SkyDome,
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().uv(32, 16))),
        MeshMaterial3d(sky),
        Transform::from_scale(Vec3::splat(20_000.0)),
        NoFrustumCulling,
    ));
    commands.spawn((
        Sun,
        DirectionalLight { shadow_maps_enabled: true, illuminance: 1.0, ..default() },
        CascadeShadowConfigBuilder {
            num_cascades: 4,
            first_cascade_far_bound: 20.0,
            maximum_distance: 200.0,
            ..default()
        }
        .build(),
    ));
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 75f32.to_radians(), near: 0.05, ..default() }),
        Hdr,
        // No depth prepass: it doubles opaque vertex work and measured slower than the
        // occlusion culling it enables, since the visibility graph already hides caves.
        Msaa::Off,
        ShadowFilteringMethod::Gaussian,
        // The display transform maps HDR to the screen instead of a tonemapper, which would
        // wash out and desaturate sunlit blocks.
        Tonemapping::None,
        DisplayTransform::default(),
        Bloom { intensity: 0.1, ..Bloom::NATURAL },
        AmbientLight { brightness: 1.0, ..default() },
        DistanceFog::default(),
        Transform::from_xyz(0.0, 100.0, 0.0),
    ));
}

/// A square texture array with an optional mip chain.
fn array_image(layers: &[Rgba], mipmaps: bool, address: ImageAddressMode) -> Image {
    let size = layers[0].width;
    let levels = if mipmaps { size.trailing_zeros() + 1 } else { 1 };
    let mut data = Vec::new();
    for layer in layers {
        let coverage = alpha_coverage(&layer.pixels);
        let mut level = layer.pixels.clone();
        let mut level_size = size;
        data.extend_from_slice(&level);
        for _ in 1..levels {
            level = downsample(&level, level_size);
            level_size /= 2;
            if coverage < 1.0 {
                preserve_coverage(&mut level, coverage);
            }
            data.extend_from_slice(&level);
        }
    }

    let extent = Extent3d { width: size, height: size, depth_or_array_layers: layers.len() as u32 };
    let base_len = (size * size * 4) as usize * layers.len();
    let mut image = Image::new(
        extent,
        TextureDimension::D2,
        data[..base_len].to_vec(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.data_order = TextureDataOrder::LayerMajor;
    image.texture_descriptor.mip_level_count = levels;
    image.texture_view_descriptor =
        Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
    let filter = if mipmaps { ImageFilterMode::Nearest } else { ImageFilterMode::Linear };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address,
        address_mode_v: address,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// Fraction of texels that pass the 0.5 alpha test.
fn alpha_coverage(pixels: &[u8]) -> f32 {
    let texels = pixels.len() / 4;
    pixels.iter().skip(3).step_by(4).filter(|&&a| a >= 128).count() as f32 / texels as f32
}

/// Scales a mip level's alpha so the same fraction of texels passes the alpha test as in
/// the full-size texture. Without this, leaves thin out and vanish with distance.
fn preserve_coverage(pixels: &mut [u8], coverage: f32) {
    let (mut low, mut high) = (0.0f32, 8.0f32);
    let scaled = |scale: f32, pixels: &[u8]| {
        let texels = pixels.len() / 4;
        pixels.iter().skip(3).step_by(4).filter(|&&a| a as f32 * scale >= 128.0).count() as f32 / texels as f32
    };
    for _ in 0..12 {
        let mid = (low + high) / 2.0;
        if scaled(mid, pixels) < coverage {
            low = mid;
        } else {
            high = mid;
        }
    }
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = (*alpha as f32 * high).min(255.0) as u8;
    }
}

/// Halves an RGBA8 square, averaging colour weighted by alpha so cut-out edges do not
/// bleed dark fringes into lower mips.
fn downsample(pixels: &[u8], size: u32) -> Vec<u8> {
    let half = (size / 2).max(1);
    let mut out = Vec::with_capacity((half * half * 4) as usize);
    for y in 0..half {
        for x in 0..half {
            let mut rgb = [0u32; 3];
            let mut alpha = 0u32;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (((y * 2 + dy) * size + x * 2 + dx) * 4) as usize;
                let a = pixels[i + 3] as u32;
                for c in 0..3 {
                    rgb[c] += pixels[i + c] as u32 * a;
                }
                alpha += a;
            }
            for c in rgb {
                out.push(c.checked_div(alpha).unwrap_or(0) as u8);
            }
            out.push((alpha / 4) as u8);
        }
    }
    out
}

/// A flat 2D image for UI use, with pixel-art sampling.
pub fn ui_image(texture: &Rgba) -> Image {
    let mut image = Image::new(
        Extent3d { width: texture.width, height: texture.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        texture.pixels.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// Builds one mesh pass of a chunk. Uses 16-bit indices whenever they fit. Runs on worker
/// threads.
pub fn build_mesh(buffers: MeshBuffers) -> Mesh {
    let indices = if buffers.vertices.len() <= u16::MAX as usize + 1 {
        Indices::U16(buffers.indices.iter().map(|&i| i as u16).collect())
    } else {
        Indices::U32(buffers.indices)
    };
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(ATTRIBUTE_VOXEL, VertexAttributeValues::Uint32x2(buffers.vertices))
        .with_inserted_indices(indices)
}

/// Sun position, light colours and sky colours for a time of day (0..1, 0 = sunrise).
struct Environment {
    to_sun: Vec3,
    daylight: f32,
    light_direction: Vec3,
    light_color: Vec3,
    ambient: Vec3,
    zenith: Vec3,
    horizon: Vec3,
}

// Noon and midnight. A sunlit top face gets about 1.0 in all, so textures keep their own
// colours in daylight; shade is lit by the cooler sky alone. Night is a few dozen times
// darker than day: a faint blue moon and sky, which torches and glowstone easily outshine.
const SUN: Vec3 = Vec3::new(0.72, 0.68, 0.62);
const LOW_SUN: Vec3 = Vec3::new(0.72, 0.45, 0.25);
const MOON: Vec3 = Vec3::new(0.026, 0.034, 0.06);
const DAY_AMBIENT: Vec3 = Vec3::new(0.40, 0.45, 0.55);
const NIGHT_AMBIENT: Vec3 = Vec3::new(0.008, 0.011, 0.02);
const DAY_ZENITH: Vec3 = Vec3::new(0.22, 0.45, 0.95);
const NIGHT_ZENITH: Vec3 = Vec3::new(0.0012, 0.0018, 0.0045);
const DAY_HORIZON: Vec3 = Vec3::new(0.62, 0.78, 0.98);
const NIGHT_HORIZON: Vec3 = Vec3::new(0.005, 0.007, 0.014);
/// Warm hues of the horizon and the skylight around sunrise and sunset, of unit luminance.
const DUSK_HORIZON: Vec3 = Vec3::new(1.9, 0.78, 0.32);
const DUSK_AMBIENT: Vec3 = Vec3::new(1.3, 0.95, 0.75);
const LUMINANCE: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

impl Environment {
    fn at(day_fraction: f32) -> Self {
        // The sun crosses the sky along a slightly tilted arc.
        let angle = day_fraction * std::f32::consts::TAU;
        let to_sun = Vec3::new(angle.cos(), angle.sin(), 0.25).normalize();
        let daylight = smoothstep(-0.2, 0.15, to_sun.y);
        let dusk = (1.0 - (to_sun.y.abs() * 4.0).min(1.0)) * smoothstep(-0.25, 0.0, to_sun.y);

        // One shadow-casting light: the sun by day, the moon (opposite it) by night. Both
        // fade to zero at the horizon, so swapping between them is invisible.
        let (light_direction, light_color) = if to_sun.y > 0.0 {
            (to_sun, LOW_SUN.lerp(SUN, smoothstep(0.0, 0.35, to_sun.y)) * smoothstep(0.0, 0.2, to_sun.y))
        } else {
            (-to_sun, MOON * smoothstep(0.0, 0.2, -to_sun.y))
        };

        let zenith = fade(NIGHT_ZENITH, DAY_ZENITH, daylight);
        let horizon = tint(fade(NIGHT_HORIZON, DAY_HORIZON, daylight), DUSK_HORIZON, dusk * 0.9);
        let ambient = tint(fade(NIGHT_AMBIENT, DAY_AMBIENT, daylight), DUSK_AMBIENT, dusk * 0.5);
        Self { to_sun, daylight, light_direction, light_color, ambient, zenith, horizon }
    }

    /// The same light under cloud: a full layer hides the sun or moon, and the sky and
    /// ambient light turn grey, darker still in rain and storms. Lightning (`flash`, 0..1)
    /// lights everything up for a moment.
    fn under(mut self, weather: &Weather, flash: f32) -> Self {
        let overcast = smoothstep(0.3, 1.0, weather.cover);
        let gloom = 1.0 - 0.75 * weather.precipitation * (0.5 + 0.5 * weather.convection);
        let grey = |color: Vec3, brightness: f32| {
            color.lerp(Vec3::splat(color.dot(Vec3::new(0.2126, 0.7152, 0.0722)) * brightness), overcast)
        };
        self.light_color *= (1.0 - 0.9 * overcast) * gloom;
        self.zenith = grey(self.zenith, 1.0) * gloom;
        self.horizon = grey(self.horizon, 0.9) * gloom;
        // Cloud spreads the hidden sunlight over the whole sky, brightening the shade.
        self.ambient = grey(self.ambient, 1.0 + 0.6 * self.daylight) * gloom;
        let lightning = Vec3::new(0.7, 0.75, 1.0) * flash;
        self.ambient += lightning * 1.5;
        self.zenith += lightning * 0.4;
        self.horizon += lightning * 0.6;
        self
    }
}

/// Blends night to day colours geometrically, so brightness changes evenly to the eye
/// through dusk and dawn instead of collapsing at the very end.
fn fade(night: Vec3, day: Vec3, daylight: f32) -> Vec3 {
    night * (day / night).powf(daylight)
}

/// Shifts `color` towards `hue` by `amount`, keeping its brightness.
fn tint(color: Vec3, hue: Vec3, amount: f32) -> Vec3 {
    color.lerp(hue * color.dot(LUMINANCE), amount)
}

/// Colour of the glow around the sun in sky.wgsl, which the haze shares.
const SUN_GLOW: Vec3 = Vec3::new(1.0, 0.8, 0.55);

/// Advances time of day and moves the sun, light colours, fog and sky to match.
#[allow(clippy::too_many_arguments)]
fn update_environment(
    time: Res<Time>,
    mut cycle: ResMut<DayCycle>,
    weather: Res<ClientWeather>,
    handles: Option<Res<VoxelMaterials>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut camera: Query<(&GlobalTransform, &mut AmbientLight, &mut DistanceFog), With<Camera3d>>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
    mut dome: Query<&mut Transform, (With<SkyDome>, Without<Sun>)>,
    mut clear: ResMut<ClearColor>,
) {
    let Some(handles) = handles else { return };
    let Ok((camera, mut ambient, mut fog)) = camera.single_mut() else { return };
    cycle.ticks += time.delta_secs_f64() * voxmar_protocol::TICKS_PER_SECOND as f64;
    let days = cycle.ticks / TICKS_PER_DAY as f64;
    let shown = &weather.current;
    let flash = weather.flash();
    let env = Environment::at(days.fract() as f32).under(shown, flash);

    if let Ok((mut transform, mut light)) = sun.single_mut() {
        *transform = Transform::default().looking_to(-env.light_direction, Vec3::Y);
        light.color = Color::linear_rgb(env.light_color.x, env.light_color.y, env.light_color.z);
    }
    if let Ok(mut dome) = dome.single_mut() {
        dome.translation = camera.translation();
    }

    let mut horizon = env.horizon;
    // Haze in the shaders (see lighting.wgsl) does the gradual fading; the linear falloff
    // only fades out the edge of the distant terrain.
    let (fog_start, fog_end) = if cycle.underwater {
        let water = Vec3::new(0.04, 0.13, 0.32);
        horizon = fade(water * 0.03, water, env.daylight);
        (0.0, 40.0)
    } else {
        // Rain and, more so, snow close in the view, and humid air brings the haze nearer.
        // Every surface fades through these, so the weather thickens fog everywhere alike.
        let murk = (shown.precipitation * (0.7 + 0.6 * shown.snow)).min(1.0);
        let end = cycle.view_distance * (1.0 - 0.9 * murk.sqrt());
        (end * (0.6 - 0.57 * murk) * (1.0 - 0.4 * smoothstep(0.5, 1.0, shown.humidity)), end)
    };
    // Haze glows around the sun like the sky does, while the sun is the light, and as
    // dully as the sky's glow under thick cloud.
    let overcast = smoothstep(0.4, 1.0, shown.cover);
    let glow = if cycle.underwater || env.to_sun.y <= 0.0 {
        Vec3::ZERO
    } else {
        SUN_GLOW * (env.daylight * 2.0).min(1.0) * (1.0 - 0.7 * overcast)
    };
    let horizon_color = Color::linear_rgb(horizon.x, horizon.y, horizon.z);
    clear.0 = horizon_color;
    ambient.color = Color::linear_rgb(env.ambient.x, env.ambient.y, env.ambient.z);
    fog.color = horizon_color;
    fog.directional_light_color = Color::linear_rgb(glow.x, glow.y, glow.z);
    fog.falloff = FogFalloff::Linear { start: fog_start, end: fog_end };

    // The sky dome is the only entity using its material, so updating it is cheap.
    if let Some(mut sky) = sky_materials.get_mut(&handles.sky) {
        let seconds = (cycle.ticks / voxmar_protocol::TICKS_PER_SECOND as f64) as f32 % 100_000.0;
        let position = camera.translation().as_dvec3();
        let layer = (DVec2::new(position.x, position.z) + weather.drift.as_dvec2()) / CLOUD_UNIT;
        sky.params = SkyParams {
            zenith: env.zenith.extend(1.0),
            horizon: horizon.extend(1.0),
            sun: env.to_sun.extend(env.daylight),
            clouds: Vec4::new(seconds, shown.cover, shown.convection, shown.cirrus),
            stars: Vec4::new(days.fract() as f32, (days % 1000.0) as f32, 0.0, 0.0),
            weather: layer.rem_euclid(DVec2::splat(4096.0)).as_vec2().extend(shown.precipitation).extend(flash),
            wind: shown.wind.normalize_or(Vec2::X).extend(0.0).extend(0.0),
        };
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
