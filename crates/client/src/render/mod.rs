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
use bevy::render::extract_component::ExtractComponent;
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
pub mod cloud_map;
pub mod cracks;
pub mod lod;
pub mod pack;
pub mod particles;
pub mod post;
pub mod scale;
pub mod precipitation;
pub mod terrain;
pub mod volumetric;
pub mod water;
pub mod wind;

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
            cracks::CrackPlugin,
            volumetric::VolumetricPlugin,
            cloud_map::CloudMapPlugin,
        ))
            .insert_resource(DirectionalLightShadowMap { size: ShadowQuality::default().map_size() })
            .init_resource::<DayCycle>()
            .init_resource::<Quality>()
            .init_resource::<pack::ActivePack>()
            .add_systems(Startup, cluster_lights_on_the_cpu)
            .add_systems(Update, (pack::apply_pack, apply_quality.run_if(resource_changed::<Quality>)))
            .add_systems(PostUpdate, update_environment.before(TransformSystems::Propagate));
    }
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
}

impl ShadowQuality {
    pub const ALL: [ShadowQuality; 4] = [ShadowQuality::Off, ShadowQuality::Low, ShadowQuality::Medium, ShadowQuality::High];

    pub fn label(self) -> &'static str {
        match self {
            ShadowQuality::Off => "Off",
            ShadowQuality::Low => "Low",
            ShadowQuality::Medium => "Medium",
            ShadowQuality::High => "High",
        }
    }

    /// Filter taps per shadow lookup (see shadows.wgsl): 0 none, 1 a single hard tap.
    pub fn taps(self) -> u32 {
        match self {
            ShadowQuality::Off => 0,
            ShadowQuality::Low => 1,
            ShadowQuality::Medium => 8,
            ShadowQuality::High => 16,
        }
    }

    fn map_size(self) -> usize {
        if self == ShadowQuality::Low { 1024 } else { 2048 }
    }

    /// The cascades: how many, how far the first reaches and how far shadows reach, in
    /// blocks. A short first cascade keeps texels small where shadows are seen up close.
    fn cascades(self) -> CascadeShadowConfig {
        let (num_cascades, first_cascade_far_bound, maximum_distance) = match self {
            ShadowQuality::Off | ShadowQuality::Low => (2, 16.0, 96.0),
            ShadowQuality::Medium => (3, 12.0, 160.0),
            ShadowQuality::High => (4, 12.0, 200.0),
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
    /// (wrapped every 1000 days), for the slow drift of the planets.
    pub stars: Vec4,
    /// xy: the cloud layer's offset over the camera in cloud units, z: precipitation,
    /// w: lightning flash.
    pub weather: Vec4,
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
) {
    commands.insert_resource(particles::TexturePieces::new(block_textures, layers));
    // Overlays are blended by their alpha, so their mips keep its average.
    let mut blended = vec![false; block_textures.len()];
    for &(overlay, _) in layers.iter().filter_map(|l| l.overlay.as_ref()) {
        blended[overlay as usize] = true;
    }
    let textures = images.add(array_image(block_textures, true, ImageAddressMode::Repeat, &blended));
    let layers = images.add(layer_table(layers));
    commands.insert_resource(terrain::TerrainTextures { textures, layers });
    let sky = sky_materials.add(SkyMaterial { params: SkyParams::default(), atmosphere: default() });
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
}

/// Each block texture layer's `LayerInfo`, one texel per layer: red and green the overlay's
/// layer plus one (0 for none), blue its tint, alpha the glow. The terrain shader adds the
/// glow to the texels bright enough to be the glowing part (see voxel.wgsl).
fn layer_table(layers: &[LayerInfo]) -> Image {
    let data = layers
        .iter()
        .flat_map(|layer| {
            let (overlay, tint) = layer.overlay.map_or((0, 0), |(overlay, tint)| (overlay + 1, tint as u8));
            let [low, high] = overlay.to_le_bytes();
            [low, high, tint, (layer.glow.clamp(0.0, 1.0) * 255.0).round() as u8]
        })
        .collect::<Vec<u8>>();
    let mut image = Image::new(
        Extent3d { width: layers.len().max(1) as u32, height: 1, depth_or_array_layers: 1 },
        TextureDimension::D2,
        if data.is_empty() { vec![0; 4] } else { data },
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

/// The direction towards the sun at a time of day (0..1, 0 = sunrise): a slightly tilted
/// arc across the sky.
fn sun_path(day_fraction: f32) -> Vec3 {
    let angle = day_fraction * std::f32::consts::TAU;
    Vec3::new(angle.cos(), angle.sin(), 0.25).normalize()
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
    fn at(day_fraction: f32) -> Self {
        let to_sun = sun_path(day_fraction);
        let daylight = smoothstep(-0.2, 0.15, to_sun.y);
        let dusk = (1.0 - (to_sun.y.abs() * 4.0).min(1.0)) * smoothstep(-0.25, 0.0, to_sun.y);

        // One shadow-casting light: the sun by day, the moon (opposite it) by night. Both
        // fade to zero at the horizon, so swapping between them is invisible. Its direction
        // moves in steps; its colour and the sky change smoothly.
        let stepped = sun_path(light_time(day_fraction));
        let (light_direction, light_color) = if to_sun.y > 0.0 {
            (stepped, LOW_SUN.lerp(SUN, smoothstep(0.0, 0.35, to_sun.y)) * smoothstep(0.0, 0.2, to_sun.y))
        } else {
            (-stepped, MOON * smoothstep(0.0, 0.2, -to_sun.y))
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

/// Advances time of day and moves the sun, light colours, the air and sky to match.
#[allow(clippy::too_many_arguments)]
fn update_environment(
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
) {
    let Some(sky_handle) = sky_handle else { return };
    // The camera's own transform (it has no parent), as this frame moved it: the global one
    // is still last frame's, and the cloud layer placed from it would lag a frame behind.
    let Ok((camera, mut ambient, mut fog)) = camera.single_mut() else { return };
    cycle.ticks += time.delta_secs_f64() * voxmar_protocol::TICKS_PER_SECOND as f64;
    let days = cycle.ticks / TICKS_PER_DAY as f64;
    let shown = &weather.current;
    let flash = weather.flash();
    let mut env = Environment::at(days.fract() as f32).under(shown, flash);
    env.light_color *= pack.sun * pack.sun_tint;
    env.ambient *= pack.ambient * pack.shade_tint;
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

    let mut horizon = env.horizon;
    if cycle.underwater {
        let water = Vec3::new(0.04, 0.13, 0.32);
        horizon = fade(water * 0.03, water, env.daylight);
    }
    // Haze glows around the sun like the sky does, while the sun is the light, and as
    // dully as the sky's glow under thick cloud.
    let overcast = smoothstep(0.4, 1.0, shown.cover);
    let glow = if cycle.underwater || env.to_sun.y <= 0.0 {
        Vec3::ZERO
    } else {
        SUN_GLOW * (env.daylight * 2.0).min(1.0) * (1.0 - 0.7 * overcast)
    };
    let light = atmosphere::Light { horizon, ambient: env.ambient, sun: env.light_color, glow, elevation: env.light_direction.y };
    let air = atmosphere::Air { haze: Some(air.haze), density: air.haze_density };
    let seconds = (cycle.ticks / voxmar_protocol::TICKS_PER_SECOND as f64) as f32 % 100_000.0;
    let position = camera.translation.as_dvec3();
    let layer = ((DVec2::new(position.x, position.z) + weather.drift.as_dvec2()) / CLOUD_UNIT).rem_euclid(DVec2::splat(4096.0)).as_vec2();
    let view = atmosphere::View { distance: cycle.view_distance, eye_height: camera.translation.y, underwater: cycle.underwater };
    let mut air = atmosphere::Atmosphere::new(&light, shown, &air, &pack, *quality, view);
    if !cycle.underwater {
        air = air.with_clouds(shown, layer, position, env.light_direction, seconds, flash, &pack);
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
            stars: Vec4::new(days.fract() as f32, (days % 1000.0) as f32, 0.0, 0.0),
            weather: layer.extend(shown.precipitation).extend(flash),
        };
        sky.atmosphere = air;
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
