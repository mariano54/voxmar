//! Cracks over the block being mined, in ten stages, as in Minecraft: a cube a hair larger
//! than the block, textured with the stage, that multiplies what is behind it by twice its
//! texel (Minecraft's crumbling blend). Mid grey leaves the block alone, the dark crack
//! lines darken it and their light edges lighten it, so the block keeps its own texture and
//! lighting under the cracks.
//!
//! The stages are `block/destroy_stage_0..9` from the texture packs (the player's
//! Minecraft install has them), or cracks drawn here when no pack does.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::{ImageSampler, ImageSamplerDescriptor, ImageFilterMode};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Extent3d, RenderPipelineDescriptor,
    SpecializedMeshPipelineError, TextureDataOrder, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use bevy::shader::ShaderRef;

use crate::textures::Rgba;

/// Stages from the first crack to a block about to break.
pub const STAGES: u32 = 10;

pub struct CrackPlugin;

impl Plugin for CrackPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "cracks.wgsl");
        app.add_plugins(MaterialPlugin::<CrackMaterial>::default())
            .init_resource::<Crack>()
            .add_systems(PostUpdate, show.before(TransformSystems::Propagate));
    }
}

/// The block being mined and how cracked it is, which effects set each frame.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
pub struct Crack {
    /// The block and its stage, 0..STAGES.
    pub at: Option<(IVec3, u32)>,
    /// How strongly cracks show, 0..1 (the shader pack's).
    pub strength: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct CrackMaterial {
    /// x: stage, y: strength.
    #[uniform(0)]
    params: Vec4,
    #[texture(1, dimension = "2d_array")]
    #[sampler(2)]
    stages: Handle<Image>,
}

impl Material for CrackMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/render/cracks.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
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
        // result = source × destination + destination × source: twice the product.
        let multiply = BlendComponent { src_factor: BlendFactor::Dst, dst_factor: BlendFactor::Src, operation: BlendOperation::Add };
        let keep = BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        if let Some(fragment) = descriptor.fragment.as_mut() {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState { color: multiply, alpha: keep });
            }
        }
        Ok(())
    }
}

#[derive(Component)]
struct CrackCube;

/// Makes the crack stages and the hidden cube that shows them, from the packs' stage
/// textures where all ten exist.
pub fn setup(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<CrackMaterial>,
    from_packs: Option<Vec<Rgba>>,
) {
    let stages = from_packs.filter(|s| s.len() == STAGES as usize).unwrap_or_else(|| (0..STAGES).map(drawn_stage).collect());
    let material = CrackMaterial { params: Vec4::new(0.0, 1.0, 0.0, 0.0), stages: images.add(stage_array(&stages)) };
    commands.spawn((
        CrackCube,
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(material)),
        Transform::from_scale(Vec3::splat(1.002)),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

/// Moves the cube onto the block being mined and shows its stage.
fn show(
    crack: Res<Crack>,
    mut cube: Query<(&mut Transform, &mut Visibility, &MeshMaterial3d<CrackMaterial>), With<CrackCube>>,
    mut materials: ResMut<Assets<CrackMaterial>>,
) {
    let Ok((mut transform, mut visibility, material)) = cube.single_mut() else { return };
    let Some((block, stage)) = crack.at.filter(|_| crack.strength > 0.0) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    visibility.set_if_neq(Visibility::Visible);
    transform.translation = block.as_vec3() + 0.5;
    let params = Vec4::new(stage.min(STAGES - 1) as f32, crack.strength.min(1.0), 0.0, 0.0);
    // Only when the stage changes, ten times a block, so the material is rarely rebuilt.
    if materials.get(&material.0).is_some_and(|m| m.params != params)
        && let Some(mut material) = materials.get_mut(&material.0)
    {
        material.params = params;
    }
}

/// The stages as one texture array, each resampled (nearest) to the first one's size.
fn stage_array(stages: &[Rgba]) -> Image {
    let size = stages[0].width.min(stages[0].height).max(1);
    let mut data = Vec::with_capacity((size * size * 4) as usize * stages.len());
    for stage in stages {
        for y in 0..size {
            for x in 0..size {
                let (sx, sy) = (x * stage.width / size, y * stage.height.min(stage.width) / size);
                let i = ((sy * stage.width + sx) * 4) as usize;
                data.extend_from_slice(stage.pixels.get(i..i + 4).unwrap_or(&[0; 4]));
            }
        }
    }
    let extent = Extent3d { width: size, height: size, depth_or_array_layers: stages.len() as u32 };
    // Unorm, not sRGB: the shader works on the stored values, as Minecraft blends them.
    let mut image = Image::new(extent, TextureDimension::D2, data, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    image.data_order = TextureDataOrder::LayerMajor;
    image.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        ..default()
    });
    image
}

/// Our own crack stage: dark lines a texel wide, each with a lighter texel below it as
/// Minecraft's have, spreading from the middle. Every stage keeps the cracks of the
/// stages before it.
fn drawn_stage(stage: u32) -> Rgba {
    const SIZE: i32 = 16;
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    let set = |x: i32, y: i32, value: u8, pixels: &mut Vec<u8>| {
        if (0..SIZE).contains(&x) && (0..SIZE).contains(&y) {
            let i = ((y * SIZE + x) * 4) as usize;
            // A dark texel is never overwritten by a light one.
            if pixels[i + 3] == 0 || value < pixels[i] {
                pixels[i..i + 4].copy_from_slice(&[value, value, value, 255]);
            }
        }
    };
    // Every crack, in the order it appears: a walk from near the middle, each step taken
    // at a moment 0..1 of the whole breaking; a stage shows the steps up to its moment.
    let until = (stage + 1) as f32 / STAGES as f32;
    let mut hash = 0x6372_6163u32;
    let mut random = move || {
        hash ^= hash << 13;
        hash ^= hash >> 17;
        hash ^= hash << 5;
        (hash >> 8) as f32 / (1u32 << 24) as f32
    };
    const CRACKS: u32 = 7;
    const STEPS: u32 = 9;
    for crack in 0..CRACKS {
        let start = crack as f32 / CRACKS as f32 * 0.75;
        let (mut x, mut y) = (6.0 + random() * 4.0, 6.0 + random() * 4.0);
        let angle = random() * std::f32::consts::TAU;
        let (mut dx, mut dy) = (angle.cos(), angle.sin());
        for step in 0..STEPS {
            // Every step is walked at every stage, so all stages share the same cracks.
            let moment = start + (step as f32 / STEPS as f32) * (1.0 - start) * 0.9;
            if moment <= until {
                let (px, py) = (x.round() as i32, y.round() as i32);
                set(px, py, 61, &mut pixels);
                set(px, py + 1, 155, &mut pixels);
            }
            // Wander a little, mostly onwards.
            let turn = (random() - 0.5) * 1.2;
            (dx, dy) = (dx * turn.cos() - dy * turn.sin(), dx * turn.sin() + dy * turn.cos());
            x += dx;
            y += dy;
        }
    }
    Rgba { width: SIZE as u32, height: SIZE as u32, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dark(stage: &Rgba) -> Vec<usize> {
        stage.pixels.chunks(4).enumerate().filter(|(_, p)| p[3] > 0 && p[0] < 100).map(|(i, _)| i).collect()
    }

    #[test]
    fn drawn_cracks_grow_stage_by_stage() {
        let stages: Vec<Rgba> = (0..STAGES).map(drawn_stage).collect();
        assert!(!dark(&stages[0]).is_empty());
        for pair in stages.windows(2) {
            let (before, after) = (dark(&pair[0]), dark(&pair[1]));
            assert!(before.iter().all(|i| after.contains(i)), "a crack vanished");
            assert!(after.len() >= before.len());
        }
        let last = dark(&stages[STAGES as usize - 1]).len();
        assert!(last > 3 * dark(&stages[0]).len() && last < 16 * 16 / 2, "{last} dark texels at the last stage");
    }
}
