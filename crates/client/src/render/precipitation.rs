//! Rain and snow falling around the camera.
//!
//! Every drop has a fixed random place in a box that follows the camera and falls (and
//! drifts with the wind) through it, wrapping around, so drops cost nothing on the CPU and
//! stay put in the world as the camera moves. How far rain and snow have fallen is the only
//! thing that changes per frame. A map of the highest block in each column around the
//! camera hides drops under roofs and trees and in caves; it also tells the rain sound how
//! open the sky is.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, VertexFormat,
};
use bevy::shader::ShaderRef;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::{CHUNK_SIZE, COLUMN_CHUNKS, ColumnPos, local_index};

use crate::weather::ClientWeather;
use crate::world::ClientWorld;

/// Drops in the box at the heaviest rain or snow, including the splashes the shader makes
/// of some of them.
const DROPS: u32 = 16_000;
/// The box of falling drops around the camera, in blocks.
const BOX: Vec3 = Vec3::new(40.0, 36.0, 40.0);
/// Falling speeds, in blocks per second.
const RAIN_SPEED: f32 = 14.0;
const SNOW_SPEED: f32 = 1.6;
/// Columns along each side of the height map, centred on the camera.
const MAP: i32 = 64;
const MAP_WORDS: usize = (MAP * MAP / 16) as usize;
/// Seconds between height map refreshes while the camera stays in one block.
const REFRESH: f32 = 0.5;

const ATTRIBUTE_DROP: MeshVertexAttribute = MeshVertexAttribute::new("Drop", 0x7261_696e, VertexFormat::Uint32);

pub struct PrecipitationPlugin;

impl Plugin for PrecipitationPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "precipitation.wgsl");
        app.add_plugins(MaterialPlugin::<PrecipitationMaterial>::default())
            .init_resource::<SkyMap>()
            .add_systems(Startup, spawn_drops)
            .add_systems(PostUpdate, update.after(TransformSystems::Propagate));
    }
}

#[derive(ShaderType, Clone, Copy, Debug)]
struct PrecipitationParams {
    /// xyz: the camera's position within the box (wrapped), w: seconds.
    camera: Vec4,
    /// xyz: the camera's position within its block, w: its height.
    block: Vec4,
    /// xyz: how far rain has fallen and drifted (wrapped into the box), w: amount 0..1.
    rain: Vec4,
    /// xyz: the same for snow, w: the fraction falling as snow.
    snow: Vec4,
    /// xyz: direction rain falls in, w: streak length in blocks.
    streak: Vec4,
    /// rgb: light falling on the drops.
    light: Vec4,
    /// Top of the highest block plus one in each column of the map, a byte per column.
    heights: [UVec4; MAP_WORDS],
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct PrecipitationMaterial {
    #[uniform(0)]
    params: PrecipitationParams,
}

impl Material for PrecipitationMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://voxmar/render/precipitation.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/render/precipitation.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    /// Transparent things draw back to front by distance; this puts the drops last, after
    /// water, which they always fall in front of.
    fn depth_bias(&self) -> f32 {
        1.0e9
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
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[ATTRIBUTE_DROP.at_shader_location(0)])?];
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// The height of the highest block in each column around the camera.
#[derive(Resource)]
pub struct SkyMap {
    /// Column at the map's first entry.
    origin: IVec2,
    /// Top of the highest block plus one, 0 where the column is empty or not loaded.
    heights: Vec<u8>,
    age: f32,
}

impl Default for SkyMap {
    fn default() -> Self {
        Self { origin: IVec2::MAX, heights: vec![0; (MAP * MAP) as usize], age: 0.0 }
    }
}

impl SkyMap {
    fn height(&self, column: IVec2) -> Option<u8> {
        let i = column - self.origin;
        ((0..MAP).contains(&i.x) && (0..MAP).contains(&i.y)).then(|| self.heights[(i.y * MAP + i.x) as usize])
    }

    /// How much of the sky is open above `eye`, from 0 (deep under cover) to 1 (out in
    /// the open), judged from the columns around it.
    pub fn openness(&self, eye: Vec3) -> f32 {
        let center = eye.xz().floor().as_ivec2();
        let mut open = 0.0;
        let mut total = 0.0;
        for dz in -3..=3 {
            for dx in -3..=3 {
                let weight = 1.0 / (1.0 + (dx * dx + dz * dz) as f32);
                if self.height(center + IVec2::new(dx, dz)).is_some_and(|top| eye.y + 1.0 >= top as f32) {
                    open += weight;
                }
                total += weight;
            }
        }
        open / total
    }

    /// Re-reads the columns around `camera` from the client's world.
    fn refresh(&mut self, world: &ClientWorld, camera: IVec2) {
        self.origin = camera - MAP / 2;
        self.age = 0.0;
        let low = ColumnPos::of_block(self.origin.extend(0).xzy());
        let high = ColumnPos::of_block((self.origin + MAP - 1).extend(0).xzy());
        for cz in low.0.y..=high.0.y {
            for cx in low.0.x..=high.0.x {
                let pos = ColumnPos::new(cx, cz);
                let column = world.voxels.column(pos);
                let origin = pos.origin().xz();
                let from = (origin - self.origin).max(IVec2::ZERO);
                let to = (origin + CHUNK_SIZE - self.origin).min(IVec2::splat(MAP));
                for z in from.y..to.y {
                    for x in from.x..to.x {
                        let local = self.origin + IVec2::new(x, z) - origin;
                        self.heights[(z * MAP + x) as usize] = column.map_or(0, |c| top(c, local));
                    }
                }
            }
        }
    }

    fn packed(&self) -> [UVec4; MAP_WORDS] {
        let word = |i: usize| u32::from_le_bytes(self.heights[i * 4..i * 4 + 4].try_into().unwrap());
        std::array::from_fn(|i| UVec4::new(word(i * 4), word(i * 4 + 1), word(i * 4 + 2), word(i * 4 + 3)))
    }
}

/// Top of the highest non-air block in a column of blocks, plus one; 0 if there is none.
fn top(column: &voxmar_engine::world::Column, local: IVec2) -> u8 {
    for cy in (0..COLUMN_CHUNKS).rev() {
        let chunk = &column.chunks[cy as usize];
        match chunk.as_uniform() {
            Some(BlockId::AIR) => continue,
            Some(_) => return (((cy + 1) * CHUNK_SIZE).min(255)) as u8,
            None => {}
        }
        for y in (0..CHUNK_SIZE).rev() {
            if !chunk.get(local_index(IVec3::new(local.x, y, local.y))).is_air() {
                return ((cy * CHUNK_SIZE + y + 1).min(255)) as u8;
            }
        }
    }
    0
}

#[derive(Component)]
struct Drops;

/// The drops never move on the CPU: each vertex knows its drop and corner, and the shader
/// does the rest.
fn spawn_drops(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<PrecipitationMaterial>>) {
    let vertices: Vec<u32> = (0..DROPS * 4).collect();
    let indices: Vec<u16> =
        (0..DROPS as u16).flat_map(|d| [0, 1, 2, 2, 1, 3].map(|i| d * 4 + i)).collect();
    let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(ATTRIBUTE_DROP, VertexAttributeValues::Uint32(vertices))
        .with_inserted_indices(Indices::U16(indices));
    let params = PrecipitationParams {
        camera: Vec4::ZERO,
        block: Vec4::ZERO,
        rain: Vec4::ZERO,
        snow: Vec4::ZERO,
        streak: Vec4::NEG_Y,
        light: Vec4::ZERO,
        heights: [UVec4::ZERO; MAP_WORDS],
    };
    commands.spawn((
        Drops,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(PrecipitationMaterial { params })),
        NoFrustumCulling,
        NotShadowCaster,
        Visibility::Hidden,
    ));
}

/// Advances the falling drops, refreshes the height map and lights the drops like the sky.
#[allow(clippy::too_many_arguments)]
fn update(
    time: Res<Time>,
    weather: Res<ClientWeather>,
    world: Res<ClientWorld>,
    mut map: ResMut<SkyMap>,
    camera: Query<(&GlobalTransform, &AmbientLight), With<Camera3d>>,
    mut drops: Query<(&MeshMaterial3d<PrecipitationMaterial>, &mut Visibility), With<Drops>>,
    mut materials: ResMut<Assets<PrecipitationMaterial>>,
    mut fallen: Local<(Vec3, Vec3)>,
) {
    let (Ok((camera, ambient)), Ok((handle, mut visibility))) = (camera.single(), drops.single_mut()) else { return };
    let shown = &weather.current;
    let falling = shown.precipitation > 0.01;
    visibility.set_if_neq(if falling { Visibility::Inherited } else { Visibility::Hidden });
    if !falling {
        return;
    }
    let eye = camera.translation();
    let block = eye.floor().as_ivec3();
    map.age += time.delta_secs();
    if map.origin != block.xz() - MAP / 2 || map.age > REFRESH {
        map.refresh(&world, block.xz());
    }

    let dt = time.delta_secs();
    let wind = Vec3::new(shown.wind.x, 0.0, shown.wind.y);
    let rain_velocity = wind - Vec3::Y * RAIN_SPEED;
    let snow_velocity = wind * 0.6 - Vec3::Y * SNOW_SPEED;
    let (rain, snow) = &mut *fallen;
    *rain = (*rain + rain_velocity * dt).rem_euclid(BOX);
    *snow = (*snow + snow_velocity * dt).rem_euclid(BOX);

    let Some(mut material) = materials.get_mut(&handle.0) else { return };
    let light = ambient.color.to_linear();
    material.params = PrecipitationParams {
        camera: eye.rem_euclid(BOX).extend(time.elapsed_secs_wrapped()),
        block: (eye - block.as_vec3()).extend(eye.y),
        rain: rain.extend(shown.precipitation),
        snow: snow.extend(shown.snow),
        streak: rain_velocity.normalize().extend(rain_velocity.length() * 0.045),
        light: Vec4::new(light.red, light.green, light.blue, 0.0),
        heights: map.packed(),
    };
}
