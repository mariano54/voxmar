//! GPU side of the voxel world: terrain material, texture arrays, camera, sun and sky.
//!
//! Time of day drives a real directional light (with cascaded shadow maps), the camera's
//! ambient light and the atmosphere uniform (haze, fog, shadow filtering; see
//! `atmosphere`). Shaders read those from the view bind group and that one buffer, so the
//! day/night cycle never modifies material assets (which would respecialise every chunk).

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::{CameraMainTextureUsages, Hdr};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::cluster::ClusterConfig;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap, ShadowFilteringMethod};
use bevy::math::DVec2;
use bevy::mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef};
use bevy::pbr::{DistanceFog, FogFalloff, MaterialPipeline, MaterialPipelineKey};
use bevy::render::batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport};
use bevy::render::extract_component::ExtractComponent;
use bevy::render::{RenderApp, RenderStartup, init_gpu_resource};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, TextureDataOrder,
    TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor, TextureViewDimension, VertexFormat,
};
use bevy::shader::{ShaderRef, load_shader_library};
use serde::{Deserialize, Serialize};
use voxmar_engine::block::Tint;
use voxmar_protocol::{TICKS_PER_DAY, Weather};

use crate::textures::Rgba;
use crate::weather::atmosphere::ClientAtmosphere;
use crate::weather::{ClientWeather, DRIFT_WRAP};

pub mod atmosphere;
pub mod bolts;
pub mod cloud_map;
pub mod cracks;
pub mod exposure;
pub mod frame_timing;
pub mod gpu_clock;
pub mod in_flight;
pub mod items;
pub mod lod;
pub mod night;
pub mod night_light;
pub mod pack;
pub mod particles;
pub mod post;
pub mod scale;
#[cfg(test)]
mod shader_check;
pub mod precipitation;
pub mod resolve;
pub mod terrain;
pub mod volumetric;
pub mod warm;
pub mod water;
pub mod wind;
pub(crate) mod work;

pub const ATTRIBUTE_VOXEL: MeshVertexAttribute = MeshVertexAttribute::new("Voxel", 0x766f_786d, VertexFormat::Uint32x2);
pub const ATTRIBUTE_LOD: MeshVertexAttribute = MeshVertexAttribute::new("Lod", 0x6c6f_6433, VertexFormat::Uint32x4);

const VOXEL_SHADER: &str = "embedded://voxmar/render/voxel.wgsl";
const DISTANT_SHADER: &str = "embedded://voxmar/render/lod.wgsl";
const GRASS_SHADER: &str = "embedded://voxmar/render/grass.wgsl";

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "lighting.wgsl");
        load_shader_library!(app, "wind.wgsl");
        embedded_asset!(app, "voxel.wgsl");
        embedded_asset!(app, "lod.wgsl");
        embedded_asset!(app, "grass.wgsl");
        embedded_asset!(app, "terrain/compact.wgsl");
        embedded_asset!(app, "sky.wgsl");
        embedded_asset!(app, "display.wgsl");
        app.add_plugins((
            atmosphere::AtmospherePlugin,
            terrain::TerrainPlugin,
            water::WaterPlugin,
            wind::WindPlugin,
            MaterialPlugin::<SkyMaterial>::default(),
            post::PostPlugin,
            scale::ScalePlugin,
            lod::LodPlugin,
            precipitation::PrecipitationPlugin,
            particles::ParticlePlugin,
            items::ItemPlugin,
            cracks::CrackPlugin,
            volumetric::VolumetricPlugin,
            cloud_map::CloudMapPlugin,
            bolts::BoltPlugin,
        ))
            .add_plugins((warm::WarmPlugin, resolve::ResolvePlugin, frame_timing::FrameTimingPlugin, gpu_clock::GpuClockPlugin, exposure::ExposurePlugin))
            .insert_resource(DirectionalLightShadowMap { size: ShadowQuality::default().map_size() })
            .init_resource::<DayCycle>()
            .init_resource::<SceneLight>()
            .init_resource::<night_light::NightLight>()
            .init_resource::<Quality>()
            .init_resource::<pack::ActivePack>()
            .add_systems(Startup, cluster_lights_on_the_cpu)
            .add_systems(Update, (pack::apply_pack, apply_quality.run_if(resource_changed::<Quality>)))
            .add_systems(PostUpdate, update_environment.before(TransformSystems::Propagate));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(RenderStartup, batch_on_the_cpu.after(init_gpu_resource::<GpuPreprocessingSupport>));
        }
    }
}

/// Our few Bevy meshes are batched on the CPU (see `main`), so views batch without GPU
/// preprocessing too. Otherwise Bevy files opaque meshes for indirect draws that only its
/// GPU instance builder fills, and the sky, our one opaque Bevy mesh, never draws.
fn batch_on_the_cpu(mut support: ResMut<GpuPreprocessingSupport>) {
    support.max_supported_mode = GpuPreprocessingMode::None;
}

/// Bevy sorts point and spot lights into clusters of the view on the GPU, in five render
/// passes a frame, and every pass costs about 0.2 ms of frame time on Metal however little
/// it does (see docs/rendering.md). We have no such lights, so the CPU does it for free.
fn cluster_lights_on_the_cpu(mut settings: ResMut<bevy::light::cluster::GlobalClusterSettings>) {
    settings.gpu_clustering = None;
}

/// Terrain is drawn in four passes (see `terrain`).
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TerrainPass {
    Opaque,
    /// Alpha-tested faces: leaves, glass, plants.
    Cutout,
    /// Cut-out, double-sided faces between two leaves, which cast no shadow.
    Inner,
    Translucent,
    /// Leafy sprigs sticking out of crowns, drawn only near the camera.
    Fringe,
}

/// How sun shadows are drawn: none, one sharp tap, or soft with a penumbra that widens
/// with distance from the caster (more taps, a longer reach, at higher quality).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShadowQuality {
    Off,
    Low,
    Medium,
    #[default]
    High,
    Ultra,
}

impl ShadowQuality {
    pub const ALL: [ShadowQuality; 5] =
        [ShadowQuality::Off, ShadowQuality::Low, ShadowQuality::Medium, ShadowQuality::High, ShadowQuality::Ultra];

    /// The key of its name in the settings (see `lang`).
    pub fn key(self) -> &'static str {
        match self {
            ShadowQuality::Off => "settings.value.off",
            ShadowQuality::Low => "settings.value.low",
            ShadowQuality::Medium => "settings.value.medium",
            ShadowQuality::High => "settings.value.high",
            ShadowQuality::Ultra => "settings.value.ultra",
        }
    }

    /// Filter taps per shadow lookup (see shadows.wgsl): 0 none, 1 a single hard tap.
    pub fn taps(self) -> u32 {
        match self {
            ShadowQuality::Off => 0,
            ShadowQuality::Low => 1,
            ShadowQuality::Medium => 8,
            ShadowQuality::High | ShadowQuality::Ultra => 16,
        }
    }

    /// Texels across each cascade's map. Ultra's 4096 sharpens shadows from about 30 blocks
    /// out, where 2048 shows steps along a shadow's edge, for about 1.8 ms a frame at the
    /// player's settings (Low Power Mode, a busy machine) and 268 MB of shadow maps.
    fn map_size(self) -> usize {
        match self {
            ShadowQuality::Low => 1024,
            ShadowQuality::Ultra => 4096,
            _ => 2048,
        }
    }

    /// The cascades: how many, how far the first reaches and how far shadows reach, in
    /// blocks. A short first cascade keeps texels small where shadows are seen up close.
    fn cascades(self) -> CascadeShadowConfig {
        let (num_cascades, first_cascade_far_bound, maximum_distance) = match self {
            ShadowQuality::Off | ShadowQuality::Low => (2, 16.0, 96.0),
            ShadowQuality::Medium => (3, 12.0, 160.0),
            ShadowQuality::High | ShadowQuality::Ultra => (4, 12.0, 200.0),
        };
        CascadeShadowConfigBuilder { num_cascades, first_cascade_far_bound, maximum_distance, ..default() }.build()
    }
}

/// The quality settings the renderer follows (the player's, from settings.rs).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    pub shadows: ShadowQuality,
    pub volumetric_light: bool,
}

impl Default for Quality {
    fn default() -> Self {
        Self { shadows: ShadowQuality::default(), volumetric_light: true }
    }
}

/// Applies the shadow quality to the sun's shadow maps.
fn apply_quality(quality: Res<Quality>, mut map: ResMut<DirectionalLightShadowMap>, mut sun: Query<(&mut DirectionalLight, &mut CascadeShadowConfig), With<Sun>>) {
    let shadows = quality.shadows;
    if map.size != shadows.map_size() {
        map.size = shadows.map_size();
    }
    for (mut light, mut cascades) in &mut sun {
        light.shadow_maps_enabled = shadows != ShadowQuality::Off;
        *cascades = shadows.cascades();
    }
}

/// Maps the HDR image to the screen in place of a tonemapper (see `post.rs` and
/// `display.wgsl`): colours
/// up to 0.8 pass through unchanged, so blocks keep their texture colours in daylight as in
/// Minecraft, and brighter ones (the sun, glints, light sources) roll off to white. The
/// shader pack's colour grading is applied first.
#[derive(Component, ExtractComponent, ShaderType, Clone, Copy, PartialEq)]
pub struct DisplayTransform {
    /// Scales the image before the curve.
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub warmth: f32,
}

impl Default for DisplayTransform {
    fn default() -> Self {
        Self::from(&pack::ShaderPack::default())
    }
}

impl From<&pack::ShaderPack> for DisplayTransform {
    fn from(pack: &pack::ShaderPack) -> Self {
        Self {
            exposure: pack.exposure,
            contrast: pack.contrast,
            saturation: pack.saturation,
            vibrance: pack.vibrance,
            warmth: pack.warmth,
        }
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
    /// (wrapped every 1000 days), for the planets and comets; z: fraction of the year,
    /// which turns the stars against the sun (see `night`).
    pub stars: Vec4,
    /// xyz: direction towards the moon, w: how much of it is lit, 0 (new) to 1 (full).
    pub moon: Vec4,
    /// xy: the cloud layer's offset over the camera in cloud units, z: precipitation,
    /// w: lightning flash.
    pub weather: Vec4,
    /// Light in the sky, 0..1: x rainbow (above 0.75 a double one), y halo (above 0.5
    /// with sun dogs), z aurora, w falling stars (1 a meteor shower).
    pub optics: Vec4,
}

/// Blocks per unit of the sky shader's cloud noise. The cloud layer's offset wraps every
/// 4096 units, which the wind drift's wrapping matches, so wrapping never shows.
pub(crate) const CLOUD_UNIT: f64 = DRIFT_WRAP as f64 / 4096.0;

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub params: SkyParams,
    /// The terrain's air, so fog hides the sky as it hides the land.
    #[uniform(1)]
    pub atmosphere: atmosphere::Atmosphere,
    /// The world's night sky.
    #[uniform(2)]
    pub layout: night::SkyLayout,
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
        // The sky leaves the depth cleared, so passes that read depth (water, light shafts)
        // know the view reaches it rather than something drawn.
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// The sky dome's material, which the day cycle updates.
#[derive(Resource)]
pub struct SkyHandle(pub Handle<SkyMaterial>);

/// Server time, advanced locally between updates.
#[derive(Resource, Default)]
pub struct DayCycle {
    pub ticks: f64,
    /// Fog end distance in blocks: the edge of the distant terrain.
    pub view_distance: f32,
    pub underwater: bool,
    /// Under water, the height of the water's surface above the eye.
    pub water_surface: f32,
}

#[derive(Component)]
pub struct SkyDome;

#[derive(Component)]
pub struct Sun;

/// Creates the texture arrays, materials, camera, sun and sky dome. `glow` is how brightly
/// each block texture layer glows by itself, 0..1 (see `glow_table`).
pub fn setup(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    meshes: &mut Assets<Mesh>,
    sky_materials: &mut Assets<SkyMaterial>,
    block_textures: &[Rgba],
    layers: &[LayerInfo],
    shapes: Vec<[f32; 4]>,
) {
    commands.insert_resource(particles::TexturePieces::new(block_textures, layers));
    // Overlays are blended by their alpha, so their mips keep its average.
    let mut blended = vec![false; block_textures.len()];
    for &(overlay, _) in layers.iter().filter_map(|l| l.overlay.as_ref()) {
        blended[overlay as usize] = true;
    }
    let textures = images.add(array_image(block_textures, true, ImageAddressMode::Repeat, &blended));
    let layers = images.add(layer_table(layers));
    commands.insert_resource(terrain::TerrainTextures { textures, layers, shapes: std::sync::Arc::new(shapes) });
    let sky = sky_materials.add(SkyMaterial { params: SkyParams::default(), atmosphere: default(), layout: default() });
    commands.insert_resource(SkyHandle(sky.clone()));
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
        ShadowQuality::default().cascades(),
    ));
    commands.spawn((
        // Water reads the depth of the scene behind it, and so does the volumetric light; without
        // MSAA water copies the scene into the image it draws in (see `water`).
        Camera3d { depth_texture_usages: (TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING).into(), ..default() },
        CameraMainTextureUsages::default().with(TextureUsages::COPY_DST),
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
        post::Glow { intensity: 0.1 },
        AmbientLight { brightness: 1.0, ..default() },
        DistanceFog::default(),
        // We have no point or spot lights to sort into clusters (see
        // `cluster_lights_on_the_cpu`): one cluster spares Bevy rebuilding and uploading
        // the default 3456 clusters' lists every frame.
        ClusterConfig::Single,
        Transform::from_xyz(0.0, 100.0, 0.0),
    ));
}

/// A square texture array with an optional mip chain. The mips of cut-out layers keep the
/// fraction of texels that pass the alpha test, except those `blended` by their alpha.
fn array_image(layers: &[Rgba], mipmaps: bool, address: ImageAddressMode, blended: &[bool]) -> Image {
    let size = layers[0].width;
    let levels = if mipmaps { size.trailing_zeros() + 1 } else { 1 };
    let mut data = Vec::new();
    for (i, layer) in layers.iter().enumerate() {
        let coverage = if blended.get(i).copied().unwrap_or(false) { 1.0 } else { alpha_coverage(&layer.pixels) };
        let mut level = layer.pixels.clone();
        let mut level_size = size;
        if mipmaps {
            bleed(&mut level, level_size);
        }
        data.extend_from_slice(&level);
        for _ in 1..levels {
            level = downsample(&level, level_size);
            level_size /= 2;
            bleed(&mut level, level_size);
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
    // Block textures keep crisp texels up close and blend when minified, so distant
    // blocks do not sparkle as texels drop in and out between pixels.
    let mag_filter = if mipmaps { ImageFilterMode::Nearest } else { ImageFilterMode::Linear };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address,
        address_mode_v: address,
        mag_filter,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// What the terrain shader knows of a block texture layer besides its texels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerInfo {
    /// A layer drawn over this one by its alpha, tinted with a biome colour, as Minecraft
    /// draws a grass block's side: the fringe of grass takes the colour the top has.
    pub overlay: Option<(u16, Tint)>,
    /// How brightly it glows by itself, 0..1.
    pub glow: f32,
    /// It glows by the block light passing through it, a lit room's window
    /// (`BlockDef::lit_through`).
    pub through: bool,
    /// How it varies from block to block (see `voxmar_engine::block::TextureVariety`).
    pub variety: voxmar_engine::block::TextureVariety,
    /// Its animation, if its texture is a strip of frames.
    pub animation: Option<Animation>,
    /// The other drawings of it its pack gives, if any and it does not animate.
    pub variants: Option<Variants>,
}

/// A layer's other drawings: `count` layers from `first`, one of which, or the layer
/// itself, each block face shows, picked by its position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Variants {
    pub first: u16,
    pub count: u8,
}

/// An animated layer's other frames: `frames - 1` layers from `first`, each shown for
/// `ticks` twentieths of a second after the layer itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Animation {
    pub first: u16,
    pub frames: u8,
    pub ticks: u8,
}

/// The tint byte's bit (`layer_table`) for a layer that animates.
const ANIMATED: u8 = 32;
/// The tint byte's bit for a layer with other drawings.
const VARIED: u8 = 16;
/// The glow byte's bit for a layer that glows by the block light through it.
const THROUGH: u8 = 128;

/// Each block texture layer's `LayerInfo`, a column of two texels per layer. The first:
/// red and green the overlay's layer plus one (0 for none), blue its tint in the low four
/// bits, whether it has other drawings (16), whether it animates (32), and whether the
/// layer turns (64) and mirrors (128) from block to block, alpha the glow in its low seven
/// bits and whether it glows by the light through it (`THROUGH`). The terrain
/// shader adds the glow to the texels bright enough to be the glowing part (see
/// voxel.wgsl). The second, read only for a layer that animates or has other drawings:
/// red and green its second frame's (or first other drawing's) layer, blue how many frames
/// (or drawings, itself included), alpha the twentieths of a second each frame shows.
fn layer_table(layers: &[LayerInfo]) -> Image {
    let first = layers.iter().flat_map(|layer| {
        let (overlay, tint) = layer.overlay.map_or((0, 0), |(overlay, tint)| (overlay + 1, tint as u8));
        let [low, high] = overlay.to_le_bytes();
        let variety = (layer.variety.turn as u8) << 6 | (layer.variety.mirror as u8) << 7;
        let animated = match (layer.animation, layer.variants) {
            (Some(_), _) => ANIMATED,
            (None, Some(_)) => VARIED,
            (None, None) => 0,
        };
        let glow = (layer.glow.clamp(0.0, 1.0) * 127.0).round() as u8 | if layer.through { THROUGH } else { 0 };
        [low, high, tint | animated | variety, glow]
    });
    let second = layers.iter().flat_map(|layer| {
        let (first, count, ticks) = match (layer.animation, layer.variants) {
            (Some(animation), _) => (animation.first, animation.frames, animation.ticks),
            (None, Some(variants)) => (variants.first, variants.count + 1, 1),
            (None, None) => (0, 0, 1),
        };
        let [low, high] = first.to_le_bytes();
        [low, high, count, ticks.max(1)]
    });
    let data: Vec<u8> = first.chain(second).collect();
    let mut image = Image::new(
        Extent3d { width: layers.len().max(1) as u32, height: 2, depth_or_array_layers: 1 },
        TextureDimension::D2,
        if data.is_empty() { vec![0; 8] } else { data },
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::nearest();
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

/// Gives the clear texels of an RGBA8 square the colour of the visible ones nearest them,
/// spreading outwards from them a texel at a time, and keeps them clear.
/// Filtering blends a texel with its clear neighbours when the texture is minified, and a
/// cut-out's edge passing the alpha test there took their colour, black in most textures:
/// plants seen from above or far off showed as black crosses.
fn bleed(pixels: &mut [u8], size: u32) {
    let size = size as usize;
    let visible = |p: &[u8], i: usize| p[i * 4 + 3] > 0;
    let mut filled: Vec<bool> = (0..size * size).map(|i| visible(pixels, i)).collect();
    if filled.iter().all(|&f| f) || !filled.iter().any(|&f| f) {
        return;
    }
    for _ in 0..size {
        let before = pixels.to_vec();
        let was = filled.clone();
        let mut grew = false;
        for i in (0..size * size).filter(|&i| !was[i]) {
            let (x, y) = (i % size, i / size);
            let mut sum = [0u32; 3];
            let mut count = 0;
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                    continue;
                }
                let n = ny as usize * size + nx as usize;
                if was[n] {
                    for c in 0..3 {
                        sum[c] += before[n * 4 + c] as u32;
                    }
                    count += 1;
                }
            }
            if count > 0 {
                for c in 0..3 {
                    pixels[i * 4 + c] = (sum[c] / count) as u8;
                }
                filled[i] = true;
                grew = true;
            }
        }
        if !grew {
            break;
        }
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
            // Clear all over: the plain average, of colours `bleed` gave the clear texels.
            let plain = |c: usize| (0..4).map(|k| pixels[(((y * 2 + k / 2) * size + x * 2 + k % 2) * 4) as usize + c] as u32).sum::<u32>() / 4;
            for (c, sum) in rgb.into_iter().enumerate() {
                out.push(sum.checked_div(alpha).unwrap_or_else(|| plain(c)) as u8);
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

/// Sun position, light colours and sky colours for a time of day (0..1, 0 = sunrise).
struct Environment {
    to_sun: Vec3,
    /// Towards the moon, and in w how much of it is lit.
    to_moon: Vec4,
    daylight: f32,
    /// How strongly the side of the sky towards the low sun is warmer than the side away
    /// (see `atmosphere::Light::dusk`).
    dusk: f32,
    light_direction: Vec3,
    light_color: Vec3,
    ambient: Vec3,
    zenith: Vec3,
    horizon: Vec3,
}

// Noon and midnight. A sunlit top face gets about 1.0 in all, so textures keep their own
// colours in daylight; shade is lit by the cooler sky alone. Night is a dozen or two times
// darker than day: a faint blue full moon and sky, which torches and glowstone easily
// outshine, and darker as the moon wanes, but never so dark that the ground turns black:
// on a moonless night under rain it was (Mariano's playtest, 2026-10-06). The night's
// colours are data (`night_light::NightLight`, `night_light.ron`).
const SUN: Vec3 = Vec3::new(0.72, 0.68, 0.62);
const LOW_SUN: Vec3 = Vec3::new(0.72, 0.45, 0.25);
const DAY_AMBIENT: Vec3 = Vec3::new(0.40, 0.45, 0.55);
const DAY_ZENITH: Vec3 = Vec3::new(0.22, 0.45, 0.95);
const DAY_HORIZON: Vec3 = Vec3::new(0.62, 0.78, 0.98);
/// Warm hues of the horizon and the skylight around sunrise and sunset, of unit luminance.
const DUSK_HORIZON: Vec3 = Vec3::new(1.9, 0.78, 0.32);
const DUSK_AMBIENT: Vec3 = Vec3::new(1.3, 0.95, 0.75);
const LUMINANCE: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);
/// How fast hail builds up on open ground at full hail (cover per second), and how fast it
/// melts away after: white within a few seconds of a heavy fall, gone a minute or two later.
const HAIL_SETTLING: f32 = 0.25;
const HAIL_MELTING: f32 = 0.006;
/// Water's colour where no world says otherwise (between worlds), linear RGB.
const WATER: Vec3 = Vec3::new(0.05, 0.2, 0.7);

/// The direction towards the sun at a time of day (0..1, 0 = sunrise): a slightly tilted
/// arc across the sky, lifted so the sun is up two thirds of the day (the engine's
/// `calendar::sun_vector`, which the server and creatures follow too).
fn sun_path(day_fraction: f32) -> Vec3 {
    let [x, y, z] = voxmar_engine::calendar::sun_vector(day_fraction as f64);
    Vec3::new(x as f32, y as f32, z as f32).normalize()
}

/// The sun's light moves across the sky in steps of this fraction of a day (a tenth of a
/// degree of its arc, about a third of a second at the normal pace). Between steps the
/// shadow maps are drawn from exactly the same direction, so nothing in them changes and
/// shadows stand still; each step moves a shadow cast from eight blocks up by under a
/// texel of the nearest shadow map, which the soft filter hides.
const LIGHT_STEP: f32 = 0.1 / 360.0;

/// The time of day the sun's light is drawn at: `day_fraction` to the nearest step.
fn light_time(day_fraction: f32) -> f32 {
    (day_fraction / LIGHT_STEP).round() * LIGHT_STEP
}

impl Environment {
    /// The light `days` days after the world began, its nights as `night` gives.
    fn at(days: f64, night: &night_light::NightLight) -> Self {
        let day_fraction = days.fract() as f32;
        let to_sun = sun_path(day_fraction);
        let daylight = smoothstep(-0.2, 0.15, to_sun.y);
        let dusk = (1.0 - (to_sun.y.abs() * 4.0).min(1.0)) * smoothstep(-0.25, 0.0, to_sun.y);
        let (to_moon, lit) = night::moon(days, sun_path);

        // One shadow-casting light: the sun by day, the moon by night, as much of it as is
        // lit. Each fades to nothing at the horizon, and the moon until dusk has gone, so
        // swapping between them is invisible. Its direction moves in steps; its colour and
        // the sky change smoothly.
        let (light_direction, light_color) = if to_sun.y > 0.0 {
            let stepped = sun_path(light_time(day_fraction));
            (stepped, LOW_SUN.lerp(SUN, smoothstep(0.0, 0.35, to_sun.y)) * smoothstep(0.0, 0.2, to_sun.y))
        } else {
            let stepped = night::moon(days.floor() + light_time(day_fraction) as f64, sun_path).0;
            let up = smoothstep(0.0, 0.2, to_moon.y) * smoothstep(0.0, 0.15, -to_sun.y);
            (stepped, night.moon * lit.powf(1.5) * up)
        };
        // A moonlit night's sky is brighter than a moonless one's.
        let moonlit = night.moonless + (1.0 - night.moonless) * lit * smoothstep(-0.1, 0.3, to_moon.y);

        let zenith = fade(night.zenith * moonlit, DAY_ZENITH, daylight);
        let horizon = tint(fade(night.horizon * moonlit, DAY_HORIZON, daylight), DUSK_HORIZON, dusk * 0.9);
        let ambient = tint(fade(night.ambient * moonlit, DAY_AMBIENT, daylight), DUSK_AMBIENT, dusk * 0.5);
        Self { to_sun, to_moon: to_moon.extend(lit), daylight, dusk, light_direction, light_color, ambient, zenith, horizon }
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
        // Under cloud no side of the sky is lit more than another.
        self.dusk *= 1.0 - overcast;
        self.zenith = grey(self.zenith, 1.0) * gloom;
        self.horizon = grey(self.horizon, 0.9) * gloom;
        // Cloud spreads the hidden sunlight over the whole sky, brightening the shade. At
        // night rain darkens it less: the night is dark enough already.
        let night_gloom = 1.0 - (1.0 - gloom) * (0.4 + 0.6 * self.daylight);
        self.ambient = grey(self.ambient, 1.0 + 0.6 * self.daylight) * night_gloom;
        let lightning = Vec3::new(0.7, 0.75, 1.0) * flash;
        self.ambient += lightning * 1.5;
        self.zenith += lightning * 0.4;
        self.horizon += lightning * 0.6;
        self
    }
}

/// The light on what is drawn outside the terrain's shaders (creatures, dropped items), as
/// the terrain's `shade_normal` (lighting.wgsl) lights a surface open to the sky at the same
/// sky and block light levels: the sky's ambient light, falling off steeply with the sky
/// light level, a share of the sun or moon, torchlight and a faint floor. Multiply a colour
/// by `at` to light it as the ground around it is lit.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct SceneLight {
    /// What full sky light (15) gives.
    sky: Vec3,
    /// How steeply sky light falls off with its level (the pack's).
    falloff: f32,
}

impl Default for SceneLight {
    fn default() -> Self {
        Self { sky: Vec3::ONE, falloff: 2.2 }
    }
}

/// The share of the sun's or moon's direct light a creature gets, its faces turned every
/// way, against a top face's.
const DIRECT_SHARE: f32 = 0.75;
/// lighting.wgsl's torchlight colour and brightness, and the light no place goes below.
const BLOCK_LIGHT: Vec3 = Vec3::new(1.0, 0.72, 0.42);
const BLOCK_LIGHT_GAIN: f32 = 1.4;
const LIGHT_FLOOR: Vec3 = Vec3::new(0.010, 0.010, 0.014);

impl SceneLight {
    /// The light at sky light `sky` and block light `block` (each 0..=15).
    pub fn at(&self, sky: u8, block: u8) -> LinearRgba {
        let sky = self.sky * (sky.min(15) as f32 / 15.0).powf(self.falloff);
        let level = block.min(15) as f32 / 15.0;
        let torch = BLOCK_LIGHT * (level / (2.5 - 1.5 * level)) * BLOCK_LIGHT_GAIN;
        let light = sky + torch + LIGHT_FLOOR;
        LinearRgba::rgb(light.x, light.y, light.z)
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

/// Advances time of day and moves the sun, light colours, the air and sky to match.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_environment(
    time: Res<Time>,
    mut cycle: ResMut<DayCycle>,
    weather: Res<ClientWeather>,
    sky_handle: Option<Res<SkyHandle>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut camera: Query<(&Transform, &mut AmbientLight, &mut DistanceFog), With<Camera3d>>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), (With<Sun>, Without<Camera3d>)>,
    mut dome: Query<&mut Transform, (With<SkyDome>, Without<Sun>, Without<Camera3d>)>,
    mut clear: ResMut<ClearColor>,
    (pack, quality, air, mut atmosphere): (Res<pack::ActivePack>, Res<Quality>, Res<ClientAtmosphere>, ResMut<atmosphere::Atmosphere>),
    (generator, mut water_column, mut hail_lying): (Option<Res<lod::WorldGenerator>>, Local<Option<(IVec2, Vec3)>>, Local<f32>),
    (player, mut sky_of, mut scene_light): (Option<Res<crate::player::LocalPlayer>>, Local<Option<u64>>, ResMut<SceneLight>),
    sky_open: Res<exposure::SkyOpen>,
    night: Res<night_light::NightLight>,
) {
    let Some(sky_handle) = sky_handle else { return };
    // The camera's own transform (it has no parent), as this frame moved it: the global one
    // is still last frame's, and the cloud layer placed from it would lag a frame behind.
    let Ok((camera, mut ambient, mut fog)) = camera.single_mut() else { return };
    cycle.ticks += time.delta_secs_f64() * voxmar_protocol::TICKS_PER_SECOND as f64;
    let days = cycle.ticks / TICKS_PER_DAY as f64;
    let shown = &weather.current;
    let flash = weather.flash();
    let mut env = Environment::at(days, &night).under(shown, flash);
    env.light_color *= pack.sun * pack.sun_tint;
    env.ambient *= pack.ambient * pack.shade_tint;
    scene_light.set_if_neq(SceneLight { sky: env.ambient + env.light_color * env.light_direction.y.max(0.0) * DIRECT_SHARE, falloff: pack.sky_falloff });
    // The place's sky tint, as far as the pack lets it show.
    let tint = Vec3::ONE.lerp(air.sky, pack.local_air.clamp(0.0, 1.0));
    env.zenith *= tint;
    env.horizon *= pack.haze_tint * tint;

    if let Ok((mut transform, mut light)) = sun.single_mut() {
        let rotation = Transform::default().looking_to(-env.light_direction, Vec3::Y).rotation;
        // Only a new step moves the light, so the shadow maps stay exactly as they were.
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
        light.color = Color::linear_rgb(env.light_color.x, env.light_color.y, env.light_color.z);
    }
    if let Ok(mut dome) = dome.single_mut() {
        dome.translation = camera.translation;
    }

    let horizon = env.horizon;
    // Haze glows around the sun like the sky does, while the sun is the light, and as
    // dully as the sky's glow under thick cloud.
    let overcast = smoothstep(0.4, 1.0, shown.cover);
    // Under water only while the camera is below the surface: a camera lifted far above a
    // player in water (a capture's pose) drew the sky as the deep water, black.
    if cycle.underwater && camera.translation.y > cycle.water_surface {
        cycle.underwater = false;
    }
    let glow = if cycle.underwater || env.to_sun.y <= 0.0 {
        Vec3::ZERO
    } else {
        SUN_GLOW * (env.daylight * 2.0).min(1.0) * (1.0 - 0.7 * overcast)
    };
    let across = Vec2::new(env.to_sun.x, env.to_sun.z).normalize_or_zero();
    let dusk = if cycle.underwater { 0.0 } else { env.dusk };
    let light = atmosphere::Light {
        horizon,
        ambient: env.ambient,
        sun: env.light_color,
        glow,
        elevation: env.light_direction.y,
        dusk: across.extend(dusk),
    };
    let air = atmosphere::Air { haze: Some(air.haze), density: air.haze_density };
    let seconds = (cycle.ticks / voxmar_protocol::TICKS_PER_SECOND as f64) as f32 % 100_000.0;
    let position = camera.translation.as_dvec3();
    let layer = ((DVec2::new(position.x, position.z) + weather.drift.as_dvec2()) / CLOUD_UNIT).rem_euclid(DVec2::splat(4096.0)).as_vec2();
    let underwater = cycle.underwater.then(|| atmosphere::Submerged {
        tint: water_tint(generator.as_deref(), camera.translation, &mut water_column),
        surface: cycle.water_surface,
        light: env.light_color * env.light_direction.y.max(0.0) + env.ambient,
    });
    // Mist belongs to the open air: dug down, in a cave or a room, it is gone.
    let mist_share = pack.mist_share(sky_open.shown);
    let view = atmosphere::View { distance: cycle.view_distance, eye_height: camera.translation.y, underwater, mist_share };
    let mut air = atmosphere::Atmosphere::new(&light, shown, &air, &pack, *quality, view);
    if !cycle.underwater {
        // The brightest flash inside the cloud lights the cloud around it.
        let in_cloud = weather.cloud_flashes().max_by(|a, b| a.1.total_cmp(&b.1));
        air = air.with_clouds(shown, layer, position, env.light_direction, seconds, flash, in_cloud, &pack);
    }
    // Hail lies white on open ground for a while after it falls, as a snow dusting does;
    // freezing rain glazes what it falls on with ice, which mirrors the sky as a wet surface.
    let dt = time.delta_secs();
    *hail_lying = (*hail_lying + dt * shown.precipitation * shown.hail * HAIL_SETTLING - dt * HAIL_MELTING).clamp(0.0, 1.0);
    air.ground.z = air.ground.z.max(*hail_lying * pack.frost);
    let glaze = shown.frost.min(shown.sleet * 4.0).clamp(0.0, 1.0);
    air.ground.x = air.ground.x.max(glaze * pack.wet_ground);
    // A rainbow, opposite the sun while it is up.
    if !cycle.underwater && env.to_sun.y > 0.0 {
        air.bow = env.to_sun.extend(shown.rainbow * smoothstep(0.0, 0.05, env.to_sun.y));
    }
    atmosphere.set_if_neq(air);

    let horizon_color = Color::linear_rgb(horizon.x, horizon.y, horizon.z);
    clear.0 = horizon_color;
    ambient.color = Color::linear_rgb(env.ambient.x, env.ambient.y, env.ambient.z);
    // Water reflects the horizon's colour from here (`fog.base_color`); the air itself is
    // the atmosphere uniform's.
    fog.color = horizon_color;
    fog.falloff = FogFalloff::Linear { start: air.haze_shape.z, end: air.haze_shape.w };

    // The sky dome is the only entity using its material, so updating it is cheap.
    if let Some(mut sky) = sky_materials.get_mut(&sky_handle.0) {
        sky.params = SkyParams {
            zenith: env.zenith.extend(1.0),
            horizon: horizon.extend(1.0),
            sun: env.to_sun.extend(env.daylight),
            clouds: Vec4::new(seconds, shown.cover, shown.convection, shown.cirrus),
            stars: Vec4::new(days.fract() as f32, (days % night::SKY_DAYS) as f32, voxmar_engine::calendar::year_fraction(days), 0.0),
            moon: env.to_moon,
            weather: layer.extend(shown.precipitation).extend(flash),
            optics: Vec4::new(shown.rainbow, shown.halo, shown.aurora, shown.falling_stars),
        };
        sky.atmosphere = air;
        // The world's own night sky, laid out once it is known.
        let seed = player.as_ref().filter(|p| p.joined).map_or(0, |p| p.seed);
        if *sky_of != Some(seed) {
            *sky_of = Some(seed);
            sky.layout = night::SkyLayout::of_world(seed);
        }
    }
}

/// The biome's colour of the water at `eye`, linear RGB, as the terrain's water takes it,
/// looked up again only when the eye moves to another column (`last`).
fn water_tint(generator: Option<&lod::WorldGenerator>, eye: Vec3, last: &mut Option<(IVec2, Vec3)>) -> Vec3 {
    let column = IVec2::new(eye.x.floor() as i32, eye.z.floor() as i32);
    if let Some((at, tint)) = *last
        && at == column
    {
        return tint;
    }
    let Some(world) = generator.and_then(|g| g.get()) else { return WATER };
    let [r, g, b] = world.sampler(column, column).colors(column.x, column.y).water;
    let tint = Color::srgb_u8(r, g, b).to_linear().to_vec3();
    *last = Some((column, tint));
    tint
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layer_table_keeps_tint_and_variety_apart() {
        use voxmar_engine::block::TextureVariety;
        let layers = [
            LayerInfo { overlay: Some((300, Tint::Water)), glow: 1.0, variety: TextureVariety { turn: true, mirror: false }, ..default() },
            LayerInfo {
                overlay: None,
                glow: 0.0,
                variety: TextureVariety { turn: false, mirror: true },
                animation: Some(Animation { first: 700, frames: 4, ticks: 3 }),
                variants: None,
                through: false,
            },
            LayerInfo { variants: Some(Variants { first: 900, count: 2 }), through: true, ..default() },
        ];
        let table = layer_table(&layers);
        let data = table.data.unwrap();
        assert_eq!(data[..4], [45, 1, Tint::Water as u8 | 64, 127]);
        assert_eq!(data[4..8], [0, 0, 128 | ANIMATED, 0]);
        // A window: no glow of its own, but by the light through it.
        assert_eq!(data[8..12], [0, 0, VARIED, THROUGH]);
        // The second row: where the frames or drawings are, how many, and for how long.
        assert_eq!(data[12..16], [0, 0, 0, 1]);
        assert_eq!(data[16..20], [188, 2, 4, 3]);
        assert_eq!(data[20..24], [132, 3, 3, 1]);
        assert!((Tint::Water as u8) < VARIED, "tints must fit under the variant, animation and variety bits");
    }

    #[test]
    fn clear_texels_take_the_colour_of_visible_ones_in_every_mip() {
        // 4×4, clear black except one red texel in a corner.
        let mut pixels = vec![0u8; 4 * 4 * 4];
        pixels[..4].copy_from_slice(&[200, 10, 10, 255]);
        bleed(&mut pixels, 4);
        for texel in pixels.chunks(4).skip(1) {
            assert_eq!(texel, [200, 10, 10, 0]);
        }
        // A 2×2 block with nothing visible keeps that colour rather than turning black.
        let half = downsample(&pixels, 4);
        assert_eq!(&half[12..16], [200, 10, 10, 0]);
    }
}
