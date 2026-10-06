//! Lightning bolts: camera-facing ribbons along each bolt's channel and its forks, added to
//! the image bright enough (HDR) that bloom gives them their glow. The weather says where
//! they are (`ClientWeather::bolts`); the mesh is rebuilt only the frames a bolt shows.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::weather::ClientWeather;

/// Ribbon widths in blocks, for the channel and its forks.
const CHANNEL_WIDTH: f32 = 0.5;
const BRANCH_WIDTH: f32 = 0.25;
/// Least width, as a share of the distance: about two pixels, so a far bolt stays a line
/// rather than breaking into dashes between pixels.
const LEAST_WIDTH: f32 = 0.0025;
/// The glow around the channel: how many times wider, and how bright.
const GLOW_WIDTH: f32 = 10.0;
const GLOW: f32 = 0.05;

pub struct BoltPlugin;

impl Plugin for BoltPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "bolts.wgsl");
        app.add_plugins(MaterialPlugin::<BoltMaterial>::default())
            .add_systems(Startup, spawn)
            .add_systems(PostUpdate, shape.after(TransformSystems::Propagate));
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Default)]
pub struct BoltMaterial {}

impl Material for BoltMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/render/bolts.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
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

/// The entity drawing every bolt now.
#[derive(Component)]
struct Bolts;

fn spawn(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<BoltMaterial>>) {
    // Never empty: the renderer's mesh allocator cannot hold a mesh with no vertices.
    let mesh = meshes.add(Ribbons { positions: vec![[0.0; 3]; 3], uvs: vec![[0.0; 2]; 3], indices: vec![0, 1, 2] }.mesh());
    let material = materials.add(BoltMaterial::default());
    // Hidden until the first strike, which would build the additive pipeline that frame.
    super::warm::warm(&mut commands, mesh.clone(), material.clone(), false, ());
    commands.spawn((
        Bolts,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        Visibility::Hidden,
        NoFrustumCulling,
    ));
}

/// Builds the ribbons facing the camera as it is this frame, or hides them.
fn shape(
    weather: Res<ClientWeather>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut bolts: Query<(&Mesh3d, &mut Visibility), With<Bolts>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Ok(camera), Ok((mesh, mut visibility))) = (camera.single(), bolts.single_mut()) else { return };
    let mut ribbons = Ribbons::default();
    let eye = camera.translation();
    for bolt in weather.bolts() {
        // The channel, and the air around it lit by it, a wide faint glow.
        ribbons.add(&bolt.channel, CHANNEL_WIDTH, bolt.brightness, eye);
        ribbons.add(&bolt.channel, CHANNEL_WIDTH * GLOW_WIDTH, bolt.brightness * GLOW, eye);
        for branch in &bolt.branches {
            ribbons.add(branch, BRANCH_WIDTH, bolt.brightness * 0.5, eye);
        }
    }
    if ribbons.positions.is_empty() {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    }
    visibility.set_if_neq(Visibility::Inherited);
    // The mesh lives only in the render world once drawn, so it is replaced whole.
    let Some(mut mesh) = meshes.get_mut(&mesh.0) else { return };
    *mesh = ribbons.mesh();
}

/// Quads along polylines, each turned to face the eye. A vertex's uv holds where across the
/// ribbon it is (x, -1..1) and the bolt's brightness (y).
#[derive(Default)]
struct Ribbons {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Ribbons {
    fn mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_indices(Indices::U32(self.indices))
    }

    fn add(&mut self, points: &[Vec3], width: f32, brightness: f32, eye: Vec3) {
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let middle = (a + b) * 0.5;
            let along = (b - a).normalize_or_zero();
            let half = 0.5 * width.max(LEAST_WIDTH * middle.distance(eye) * width / CHANNEL_WIDTH);
            let side = along.cross(eye - middle).normalize_or_zero() * half;
            let base = self.positions.len() as u32;
            for (point, across) in [(a - side, -1.0), (a + side, 1.0), (b + side, 1.0), (b - side, -1.0)] {
                self.positions.push(point.to_array());
                self.uvs.push([across, brightness]);
            }
            self.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ribbons_face_the_eye() {
        let mut ribbons = Ribbons::default();
        let eye = Vec3::new(0.0, 0.0, 10.0);
        ribbons.add(&[Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO, Vec3::new(1.0, -5.0, 0.0)], 0.5, 0.8, eye);
        assert_eq!(ribbons.positions.len(), 8);
        assert_eq!(ribbons.indices.len(), 12);
        // The first quad spans the vertical segment sideways (along x), half a block wide,
        // facing the eye on the z axis.
        let [l, r] = [ribbons.positions[0], ribbons.positions[1]].map(Vec3::from_array);
        assert!((l.distance(r) - 0.5).abs() < 1e-5 && (l - r).z.abs() < 1e-5 && (l - r).y.abs() < 1e-5);
        assert_eq!(ribbons.uvs[1], [1.0, 0.8]);
    }
}
