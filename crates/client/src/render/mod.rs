//! GPU side of the voxel world: terrain material, texture arrays, sky and day/night.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, SpecializedMeshPipelineError, TextureDataOrder, TextureDimension, TextureFormat,
    TextureViewDescriptor, TextureViewDimension, VertexFormat,
};
use bevy::shader::ShaderRef;
use voxmar_engine::mesh::MeshBuffers;
use voxmar_protocol::TICKS_PER_DAY;

use crate::textures::Rgba;

pub const ATTRIBUTE_VOXEL: MeshVertexAttribute = MeshVertexAttribute::new("Voxel", 0x766f_786d, VertexFormat::Uint32x2);

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "voxel.wgsl");
        embedded_asset!(app, "sky.wgsl");
        app.add_plugins((MaterialPlugin::<VoxelMaterial>::default(), MaterialPlugin::<SkyMaterial>::default()))
            .init_resource::<DayCycle>()
            .add_systems(PostUpdate, update_environment.before(TransformSystems::Propagate));
    }
}

#[derive(ShaderType, Clone, Copy, Default, Debug)]
pub struct VoxelParams {
    pub sun: Vec4,
    pub fog_color: Vec4,
    pub fog: Vec4,
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
    #[uniform(4)]
    pub params: VoxelParams,
    pub translucent: bool,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoxelMaterialKey {
    translucent: bool,
}

impl From<&VoxelMaterial> for VoxelMaterialKey {
    fn from(material: &VoxelMaterial) -> Self {
        Self { translucent: material.translucent }
    }
}

impl Material for VoxelMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://voxmar/render/voxel.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/render/voxel.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.translucent { AlphaMode::Blend } else { AlphaMode::Opaque }
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[ATTRIBUTE_VOXEL.at_shader_location(0)])?];
        if key.bind_group_data.translucent {
            // Water is visible from below too.
            descriptor.primitive.cull_mode = None;
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("TRANSLUCENT".into());
            }
        }
        Ok(())
    }
}

#[derive(ShaderType, Clone, Copy, Default, Debug)]
pub struct SkyParams {
    pub zenith: Vec4,
    pub horizon: Vec4,
    pub sun: Vec4,
}

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
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// Handles to the shared terrain materials.
#[derive(Resource)]
pub struct VoxelMaterials {
    pub opaque: Handle<VoxelMaterial>,
    pub translucent: Handle<VoxelMaterial>,
    pub sky: Handle<SkyMaterial>,
}

/// Server time, advanced locally between updates.
#[derive(Resource, Default)]
pub struct DayCycle {
    pub ticks: f64,
    /// Fog end distance in blocks.
    pub view_distance: f32,
    pub underwater: bool,
}

#[derive(Component)]
pub struct SkyDome;

/// Creates the texture arrays, materials and sky dome.
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
    let material = |translucent| VoxelMaterial {
        textures: textures.clone(),
        colormaps: colormaps.clone(),
        params: VoxelParams::default(),
        translucent,
    };
    let sky = sky_materials.add(SkyMaterial { params: SkyParams::default() });
    commands.insert_resource(VoxelMaterials {
        opaque: voxel_materials.add(material(false)),
        translucent: voxel_materials.add(material(true)),
        sky: sky.clone(),
    });
    commands.spawn((
        SkyDome,
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().uv(32, 16))),
        MeshMaterial3d(sky),
        Transform::from_scale(Vec3::splat(4000.0)),
        NoFrustumCulling,
    ));
}

/// A square texture array with an optional box-filtered mip chain.
fn array_image(layers: &[Rgba], mipmaps: bool, address: ImageAddressMode) -> Image {
    let size = layers[0].width;
    let levels = if mipmaps { size.trailing_zeros() + 1 } else { 1 };
    let mut data = Vec::new();
    for layer in layers {
        let mut level = layer.pixels.clone();
        let mut level_size = size;
        data.extend_from_slice(&level);
        for _ in 1..levels {
            level = downsample(&level, level_size);
            level_size /= 2;
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
                out.push(if alpha == 0 { 0 } else { (c / alpha) as u8 });
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

/// Uploads one mesh pass of a chunk. Uses 16-bit indices whenever they fit.
pub fn build_mesh(buffers: &MeshBuffers) -> Mesh {
    let indices = if buffers.vertices.len() <= u16::MAX as usize + 1 {
        Indices::U16(buffers.indices.iter().map(|&i| i as u16).collect())
    } else {
        Indices::U32(buffers.indices.clone())
    };
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(ATTRIBUTE_VOXEL, VertexAttributeValues::Uint32x2(buffers.vertices.clone()))
        .with_inserted_indices(indices)
}

/// Advances time of day and pushes sun, sky and fog parameters to the materials.
fn update_environment(
    time: Res<Time>,
    mut cycle: ResMut<DayCycle>,
    handles: Option<Res<VoxelMaterials>>,
    mut voxel_materials: ResMut<Assets<VoxelMaterial>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut dome: Query<&mut Transform, With<SkyDome>>,
    mut clear: ResMut<ClearColor>,
) {
    let Some(handles) = handles else { return };
    cycle.ticks += time.delta_secs_f64() * voxmar_protocol::TICKS_PER_SECOND as f64;
    let day_fraction = (cycle.ticks / TICKS_PER_DAY as f64).fract() as f32;

    // Tick 0 is sunrise; the sun crosses the sky along a slightly tilted arc.
    let angle = day_fraction * std::f32::consts::TAU;
    let sun = Vec3::new(angle.cos(), angle.sin(), 0.25).normalize();
    let daylight = smoothstep(-0.12, 0.2, sun.y);
    let dusk = (1.0 - (sun.y.abs() * 4.0).min(1.0)) * smoothstep(-0.25, 0.0, sun.y);

    let day_zenith = Vec3::new(0.25, 0.48, 0.95);
    let day_horizon = Vec3::new(0.66, 0.80, 0.98);
    let night_zenith = Vec3::new(0.004, 0.006, 0.02);
    let night_horizon = Vec3::new(0.02, 0.03, 0.06);
    let sunset = Vec3::new(1.0, 0.45, 0.2);
    let zenith = night_zenith.lerp(day_zenith, daylight);
    let mut horizon = night_horizon.lerp(day_horizon, daylight).lerp(sunset, dusk * 0.6);
    if cycle.underwater {
        horizon = Vec3::new(0.05, 0.15, 0.35) * (0.2 + 0.8 * daylight);
    }

    clear.0 = Color::linear_rgb(horizon.x, horizon.y, horizon.z);
    let params = VoxelParams {
        sun: sun.extend(daylight),
        fog_color: horizon.extend(1.0),
        fog: Vec4::new(cycle.view_distance * 0.55, cycle.view_distance, time.elapsed_secs(), cycle.underwater as u32 as f32),
    };
    for handle in [&handles.opaque, &handles.translucent] {
        if let Some(mut material) = voxel_materials.get_mut(handle) {
            material.params = params;
        }
    }
    if let Some(mut sky) = sky_materials.get_mut(&handles.sky) {
        sky.params = SkyParams { zenith: zenith.extend(1.0), horizon: horizon.extend(1.0), sun: sun.extend(daylight) };
    }
    if let (Ok(camera), Ok(mut dome)) = (camera.single(), dome.single_mut()) {
        dome.translation = camera.translation();
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
