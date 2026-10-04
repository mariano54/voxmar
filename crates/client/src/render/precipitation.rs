//! Rain, drizzle, snow, sleet and hail falling around the camera, and dust and glowing
//! motes the wind carries through the air.
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
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::coords::{CHUNK_SIZE, COLUMN_CHUNKS, ColumnPos, local_index};

use crate::weather::ClientWeather;
use crate::world::ClientWorld;

/// Drops in the box at the heaviest rain or snow, including the splashes the shader makes
/// of some of them and the specks of dust and motes it carries on the wind, and the box of
/// falling drops around the camera, in blocks. The shader has the same constants.
const DROPS: u32 = 20_000;
const BOX: Vec3 = Vec3::new(40.0, 36.0, 40.0);
/// Falling speeds, in blocks per second.
const RAIN_SPEED: f32 = 14.0;
const SNOW_SPEED: f32 = 1.6;
const DRIZZLE_SPEED: f32 = 4.0;
const SLEET_SPEED: f32 = 8.0;
const HAIL_SPEED: f32 = 18.0;
/// Columns along each side of the height map around the camera. It is re-read when the
/// camera strays this many blocks from its centre (it still covers the drop box then), and
/// every `REFRESH` seconds for blocks that changed.
const MAP: i32 = 64;
const MAP_WORDS: usize = (MAP * MAP / 8) as usize;
const RECENTER: i32 = 8;
const REFRESH: f32 = 1.0;

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
    /// xz: the camera's position from the height map's corner, w: its height.
    map: Vec4,
    /// xyz: how far rain has fallen and drifted (wrapped into the box), w: amount 0..1.
    rain: Vec4,
    /// xyz: the same for snow, w: the fraction falling as snow.
    snow: Vec4,
    /// xyz: direction rain falls in, w: streak length in blocks.
    streak: Vec4,
    /// rgb: light falling on the drops.
    light: Vec4,
    /// Of the rain that is not snow: xyz how far drizzle, sleet and hail have fallen, w
    /// the fraction of it that is each.
    drizzle: Vec4,
    sleet: Vec4,
    hail: Vec4,
    /// xyz: how far dust and motes have drifted, w: how much of each the air holds.
    dust: Vec4,
    motes: Vec4,
    /// rgb: the dust's colour (linear); w: unused.
    dust_color: Vec4,
    /// xyz: the wind, in blocks per second.
    wind: Vec4,
    /// Top of the highest block plus one in each column of the map, 16 bits per column.
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
    heights: Vec<u16>,
    age: f32,
}

impl Default for SkyMap {
    fn default() -> Self {
        Self { origin: IVec2::MAX, heights: vec![0; (MAP * MAP) as usize], age: 0.0 }
    }
}

impl SkyMap {
    fn height(&self, column: IVec2) -> Option<u16> {
        let i = column - self.origin;
        ((0..MAP).contains(&i.x) && (0..MAP).contains(&i.y)).then(|| self.heights[(i.y * MAP + i.x) as usize])
    }

    /// How well rain can be heard at `eye`, from the columns around it: 1 out in the open,
    /// less under a roof, nothing deep underground.
    pub fn openness(&self, eye: Vec3) -> f32 {
        let center = eye.xz().floor().as_ivec2();
        let (mut heard, mut total) = (0.0, 0.0);
        for dz in -3..=3 {
            for dx in -3..=3 {
                let weight = 1.0 / (1.0 + (dx * dx + dz * dz) as f32);
                let cover = self.height(center + IVec2::new(dx, dz)).map_or(0.0, |top| top as f32 - (eye.y + 1.0));
                heard += weight * if cover <= 0.0 { 1.0 } else { 0.3 * (1.0 - (cover / 16.0).min(1.0)) };
                total += weight;
            }
        }
        heard / total
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
                let registry = &world.registry;
                let origin = pos.origin().xz();
                let from = (origin - self.origin).max(IVec2::ZERO);
                let to = (origin + CHUNK_SIZE - self.origin).min(IVec2::splat(MAP));
                for z in from.y..to.y {
                    for x in from.x..to.x {
                        let local = self.origin + IVec2::new(x, z) - origin;
                        self.heights[(z * MAP + x) as usize] = column.map_or(0, |c| top(c, registry, local));
                    }
                }
            }
        }
    }

    fn packed(&self) -> [UVec4; MAP_WORDS] {
        let word = |i: usize| self.heights[i * 2] as u32 | (self.heights[i * 2 + 1] as u32) << 16;
        std::array::from_fn(|i| UVec4::new(word(i * 4), word(i * 4 + 1), word(i * 4 + 2), word(i * 4 + 3)))
    }
}

/// Top of the highest block in a column that rain lands on, plus one; 0 if there is none.
/// Rain falls through plants onto the ground they grow from.
fn top(column: &voxmar_engine::world::Column, registry: &BlockRegistry, local: IVec2) -> u16 {
    let catches_rain = |block: BlockId| !matches!(registry.info(block).render, RenderKind::Invisible | RenderKind::Plant);
    for cy in (0..COLUMN_CHUNKS).rev() {
        let chunk = &column.chunks[cy as usize];
        match chunk.as_uniform() {
            Some(block) if !catches_rain(block) => continue,
            Some(_) => return ((cy + 1) * CHUNK_SIZE) as u16,
            None => {}
        }
        for y in (0..CHUNK_SIZE).rev() {
            if catches_rain(chunk.get(local_index(IVec3::new(local.x, y, local.y)))) {
                return (cy * CHUNK_SIZE + y + 1) as u16;
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
    let indices: Vec<u32> = (0..DROPS).flat_map(|d| [0, 1, 2, 2, 1, 3].map(|i| d * 4 + i)).collect();
    let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(ATTRIBUTE_DROP, VertexAttributeValues::Uint32(vertices))
        .with_inserted_indices(Indices::U32(indices));
    let params = PrecipitationParams {
        camera: Vec4::ZERO,
        map: Vec4::ZERO,
        rain: Vec4::ZERO,
        snow: Vec4::ZERO,
        streak: Vec4::NEG_Y,
        light: Vec4::ZERO,
        drizzle: Vec4::ZERO,
        sleet: Vec4::ZERO,
        hail: Vec4::ZERO,
        dust: Vec4::ZERO,
        motes: Vec4::ZERO,
        dust_color: Vec4::ZERO,
        wind: Vec4::ZERO,
        heights: [UVec4::ZERO; MAP_WORDS],
    };
    commands.spawn((
        Drops,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(PrecipitationMaterial { params })),
        NoFrustumCulling,
        NotShadowCaster,
    ));
}

/// How far each kind of drop has fallen and drifted, wrapped into the box.
#[derive(Default)]
struct Fallen {
    rain: Vec3,
    snow: Vec3,
    drizzle: Vec3,
    sleet: Vec3,
    hail: Vec3,
    dust: Vec3,
    motes: Vec3,
}

/// Advances the falling drops, refreshes the height map and lights the drops like the sky.
#[allow(clippy::too_many_arguments)]
fn update(
    time: Res<Time>,
    weather: Res<ClientWeather>,
    world: Res<ClientWorld>,
    mut map: ResMut<SkyMap>,
    camera: Query<(&GlobalTransform, &AmbientLight), With<Camera3d>>,
    drops: Query<&MeshMaterial3d<PrecipitationMaterial>, With<Drops>>,
    mut materials: ResMut<Assets<PrecipitationMaterial>>,
    mut fallen: Local<Fallen>,
) {
    let (Ok((camera, ambient)), Ok(handle)) = (camera.single(), drops.single()) else { return };
    let shown = &weather.current;
    if shown.precipitation <= 0.01 && shown.dust <= 0.01 && shown.motes <= 0.01 {
        // The drops stay drawn while nothing falls, every one culled in the vertex shader, so
        // their pipeline is built with the world rather than when the first rain starts:
        // where pipelines are built synchronously (macOS) that would stall a frame.
        let showing = |p: &PrecipitationParams| p.rain.w > 0.0 || p.dust.w > 0.0 || p.motes.w > 0.0;
        if materials.get(&handle.0).is_some_and(|m| showing(&m.params))
            && let Some(mut material) = materials.get_mut(&handle.0)
        {
            material.params.rain.w = 0.0;
            material.params.dust.w = 0.0;
            material.params.motes.w = 0.0;
        }
        return;
    }
    let eye = camera.translation();
    let column = eye.xz().floor().as_ivec2();
    map.age += time.delta_secs();
    if (column - (map.origin + MAP / 2)).abs().max_element() > RECENTER || map.age > REFRESH {
        map.refresh(&world, column);
    }

    let dt = time.delta_secs();
    let wind = Vec3::new(shown.wind.x, 0.0, shown.wind.y);
    let rain_velocity = wind - Vec3::Y * RAIN_SPEED;
    let snow_velocity = wind * 0.6 - Vec3::Y * SNOW_SPEED;
    let fallen = &mut *fallen;
    let fall = |at: &mut Vec3, velocity: Vec3| *at = (*at + velocity * dt).rem_euclid(BOX);
    fall(&mut fallen.rain, rain_velocity);
    fall(&mut fallen.snow, snow_velocity);
    // Fine drizzle drifts with the wind, sleet less so, and hail falls hard.
    fall(&mut fallen.drizzle, wind * 0.9 - Vec3::Y * DRIZZLE_SPEED);
    fall(&mut fallen.sleet, wind * 0.7 - Vec3::Y * SLEET_SPEED);
    fall(&mut fallen.hail, wind * 0.3 - Vec3::Y * HAIL_SPEED);
    // Dust rides the wind, settling slowly; motes drift and rise.
    fall(&mut fallen.dust, wind - Vec3::Y * 0.3);
    fall(&mut fallen.motes, wind * 0.15 + Vec3::Y * 0.4);

    let Some(mut material) = materials.get_mut(&handle.0) else { return };
    let light = ambient.color.to_linear();
    material.params = PrecipitationParams {
        camera: eye.rem_euclid(BOX).extend(time.elapsed_secs_wrapped()),
        map: Vec4::new(eye.x - map.origin.x as f32, 0.0, eye.z - map.origin.y as f32, eye.y),
        rain: fallen.rain.extend(shown.precipitation),
        snow: fallen.snow.extend(shown.snow),
        streak: rain_velocity.normalize().extend(rain_velocity.length() * 0.045),
        light: Vec4::new(light.red, light.green, light.blue, 0.0),
        drizzle: fallen.drizzle.extend(shown.drizzle),
        sleet: fallen.sleet.extend(shown.sleet),
        hail: fallen.hail.extend(shown.hail),
        dust: fallen.dust.extend(shown.dust),
        motes: fallen.motes.extend(shown.motes.max(shown.wisps * 0.3)),
        dust_color: Color::srgb_u8(shown.dust_color[0], shown.dust_color[1], shown.dust_color[2]).to_linear().to_vec4(),
        wind: wind.extend(0.0),
        heights: map.packed(),
    };
}
