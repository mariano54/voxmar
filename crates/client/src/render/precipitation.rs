//! Rain, drizzle, snow, sleet and hail falling around the camera, and dust and glowing
//! motes the wind carries through the air.
//!
//! Every drop has a fixed random place in a box that follows the camera and falls (and
//! drifts with the wind) through it, wrapping around, so drops cost nothing on the CPU and
//! stay put in the world as the camera moves. How far rain and snow have fallen is the only
//! thing that changes per frame. A map of the highest block in each column around the
//! camera hides drops under roofs and trees and in caves; it also tells the rain sound how
//! open the sky is. The map knows which columns are water, where raindrops ring out as
//! ripples rather than splash, and which are under leaves, where rain comes through as
//! slow, heavy drips that carry on for a while after the rain stops. A sparser layer of
//! fainter streaks in a box three times as wide carries the rain into the distance.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{DistanceFog, MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, VertexFormat,
};
use bevy::shader::ShaderRef;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind, Sound};
use voxmar_engine::coords::{CHUNK_SIZE, COLUMN_CHUNKS, ColumnPos, local_index};
use voxmar_engine::weather::Weather;

use crate::weather::ClientWeather;
use crate::weather::atmosphere::ClientAtmosphere;
use voxmar_worldgen::Motion;
use crate::world::ClientWorld;

/// Drops in the box at the heaviest rain or snow, including the splashes the shader makes
/// of some of them and the specks of dust and motes it carries on the wind, and the box of
/// falling drops around the camera, in blocks. The shader has the same constants.
const DROPS: u32 = 74_000;
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
const MAP_WORDS: usize = (MAP * MAP / 4) as usize;
/// A column of the map: the top of the highest block rain lands on, plus one (bits 0..10),
/// the same for the ground beneath any leaves (bits 10..20), and whether the top is water
/// or leaves.
const TOP_BITS: u32 = 0x3ff;
const WATER: u32 = 1 << 20;
const LEAVES: u32 = 1 << 21;
const RECENTER: i32 = 8;
const REFRESH: f32 = 1.0;

/// Ambient particle sets drawn at once: the densest of `ClientAtmosphere::particles`.
const AMBIENT_SETS: usize = 4;

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
    /// xyz: how far snow has fallen less where the camera is (wrapped into its box, 0.6 as
    /// wide as the rain's), w: the fraction falling as snow.
    snow: Vec4,
    /// xyz: direction rain falls in, w: streak length in blocks.
    streak: Vec4,
    /// rgb: light falling on the drops; w: how heavy the rain, snow or dust is, 0..1.
    light: Vec4,
    /// rgb: the colour of the sky's haze, which rain takes on in part.
    haze: Vec4,
    /// xyz: how far the distant layer of rain has fallen less where the camera is (wrapped
    /// into a box three times as wide), w: how many drips fall from leaves.
    far: Vec4,
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
    /// Up to `AMBIENT_SETS` sets of ambient particles, three entries each: xyz how far
    /// they have drifted, w how many; rgb their colour (linear), w 1 if they glow; x their
    /// size in blocks, y how they move (see `motion_code`).
    ambient: [Vec4; 3 * AMBIENT_SETS],
    /// Each column of the map, as `TOP_BITS`, `WATER` and `LEAVES` describe.
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
    /// Each column, as `TOP_BITS`, `WATER` and `LEAVES` describe; 0 where it is empty or
    /// not loaded.
    heights: Vec<u32>,
    age: f32,
}

impl Default for SkyMap {
    fn default() -> Self {
        Self { origin: IVec2::MAX, heights: vec![0; (MAP * MAP) as usize], age: 0.0 }
    }
}

impl SkyMap {
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
        std::array::from_fn(|i| UVec4::from_slice(&self.heights[i * 4..i * 4 + 4]))
    }
}

/// A column of the map (see `TOP_BITS`): where rain lands in it, where it lands beneath
/// any leaves, and whether it lands on water or leaves. Rain falls through plants onto the
/// ground they grow from.
fn top(column: &voxmar_engine::world::Column, registry: &BlockRegistry, local: IVec2) -> u32 {
    let catches_rain = |block: BlockId| !matches!(registry.info(block).render, RenderKind::Invisible | RenderKind::Plant);
    let leaves = |block: BlockId| registry.info(block).render == RenderKind::Leaves;
    // The highest block below `from` rain lands on (passing through leaves if asked), and
    // the top of it plus one. Uniform chunks it would pass through are skipped whole.
    let below = |from: i32, through_leaves: bool| -> Option<(u32, BlockId)> {
        let lands = |block: BlockId| catches_rain(block) && !(through_leaves && leaves(block));
        let mut y = from - 1;
        while y >= 0 {
            let (cy, ly) = (y / CHUNK_SIZE, y % CHUNK_SIZE);
            let chunk = &column.chunks[cy as usize];
            if !chunk.as_uniform().is_some_and(|b| !lands(b)) {
                for ly in (0..=ly).rev() {
                    let block = chunk.get(local_index(IVec3::new(local.x, ly, local.y)));
                    if lands(block) {
                        return Some(((cy * CHUNK_SIZE + ly + 1) as u32, block));
                    }
                }
            }
            y = cy * CHUNK_SIZE - 1;
        }
        None
    };
    let Some((top, block)) = below(COLUMN_CHUNKS * CHUNK_SIZE, false) else { return 0 };
    if leaves(block) {
        let ground = below(top as i32 - 1, true).map_or(0, |(y, _)| y);
        (top & TOP_BITS) | (ground & TOP_BITS) << 10 | LEAVES
    } else {
        let water = if registry.info(block).sound == Sound::Water { WATER } else { 0 };
        (top & TOP_BITS) | (top & TOP_BITS) << 10 | water
    }
}

/// The drops' kinds, as ranges of their indices (as in precipitation.wgsl): the falling
/// drops (with the distant rain and drips from leaves), then specks of dust, motes, the
/// ambient particle sets, and the splashes.
const SPLASHES: u32 = 3000;
const DUST: u32 = 9200;
const MOTES: u32 = 800;
const AMBIENT: u32 = 1500;
const AIRBORNE: u32 = DROPS - SPLASHES - DUST - MOTES - AMBIENT_SETS as u32 * AMBIENT;

/// One kind of drops, drawn only while it shows: undrawn drops cost the vertex shader
/// too, and in fair weather all 74,000 were drawn to show a few specks of pollen.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Drops {
    Falling,
    Dust,
    Motes,
    /// One ambient particle set (of `AMBIENT_SETS`), drawn only while that set shows: most
    /// places have one or two.
    Ambient(u8),
    Splashes,
    /// One drop, always drawn (and in fair weather hidden by the shader), so the drops'
    /// pipeline is built with the world rather than when the first rain starts: where
    /// pipelines are built synchronously (macOS) that would stall a frame.
    Warm,
}

impl Drops {
    const ALL: [Drops; 5 + AMBIENT_SETS] = [
        Drops::Falling,
        Drops::Dust,
        Drops::Motes,
        Drops::Ambient(0),
        Drops::Ambient(1),
        Drops::Ambient(2),
        Drops::Ambient(3),
        Drops::Splashes,
        Drops::Warm,
    ];

    fn range(self) -> std::ops::Range<u32> {
        match self {
            Drops::Falling => 0..AIRBORNE,
            Drops::Dust => AIRBORNE..AIRBORNE + DUST,
            Drops::Motes => AIRBORNE + DUST..AIRBORNE + DUST + MOTES,
            Drops::Ambient(set) => {
                let start = AIRBORNE + DUST + MOTES + set as u32 * AMBIENT;
                start..start + AMBIENT
            }
            Drops::Splashes => DROPS - SPLASHES..DROPS,
            Drops::Warm => 0..1,
        }
    }
}

/// The drops never move on the CPU: each vertex knows its drop and corner, and the shader
/// does the rest.
fn spawn_drops(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<PrecipitationMaterial>>) {
    let mut mesh = |drops: std::ops::Range<u32>| {
        let vertices: Vec<u32> = (drops.start * 4..drops.end * 4).collect();
        let indices: Vec<u32> = (0..drops.len() as u32).flat_map(|d| [0, 1, 2, 2, 1, 3].map(|i| d * 4 + i)).collect();
        meshes.add(
            Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(ATTRIBUTE_DROP, VertexAttributeValues::Uint32(vertices))
                .with_inserted_indices(Indices::U32(indices)),
        )
    };
    let params = PrecipitationParams {
        camera: Vec4::ZERO,
        map: Vec4::ZERO,
        rain: Vec4::ZERO,
        snow: Vec4::ZERO,
        streak: Vec4::NEG_Y,
        light: Vec4::ZERO,
        haze: Vec4::ZERO,
        far: Vec4::ZERO,
        drizzle: Vec4::ZERO,
        sleet: Vec4::ZERO,
        hail: Vec4::ZERO,
        dust: Vec4::ZERO,
        motes: Vec4::ZERO,
        dust_color: Vec4::ZERO,
        wind: Vec4::ZERO,
        ambient: [Vec4::ZERO; 3 * AMBIENT_SETS],
        heights: [UVec4::ZERO; MAP_WORDS],
    };
    let material = materials.add(PrecipitationMaterial { params });
    for drops in Drops::ALL {
        let visibility = if drops == Drops::Warm { Visibility::Inherited } else { Visibility::Hidden };
        commands.spawn((drops, Mesh3d(mesh(drops.range())), MeshMaterial3d(material.clone()), visibility, NoFrustumCulling, NotShadowCaster));
    }
}

/// How many of the drops show for an amount of rain, snow or dust (0..1): few in light
/// weather, rising steeply so that only the heaviest fills the air.
fn density(amount: f32) -> f32 {
    amount.clamp(0.0, 1.0).powf(1.8)
}

/// How far each kind of drop has fallen and drifted, wrapped into the box.
#[derive(Default)]
struct Fallen {
    rain: Vec3,
    /// The distant layer of rain, wrapped into a box three times as wide.
    far: Vec3,
    /// How the camera moved last frame, smoothed, in blocks per second.
    camera_velocity: Vec3,
    last_eye: Option<Vec3>,
    snow: Vec3,
    drizzle: Vec3,
    sleet: Vec3,
    hail: Vec3,
    dust: Vec3,
    motes: Vec3,
    /// Per ambient particle set, by name.
    ambient: Vec<(String, Vec3)>,
    /// The glinting ice crystals of diamond dust.
    glints: Vec3,
    /// Puffs of mist.
    mist: Vec3,
}

/// The shader's motion code for mist: big, faint puffs drifting slowly, kept below the top
/// of the fog layer.
const MIST: f32 = 6.0;

/// How much mist hangs in the air as puffs: ground fog (dawn mist, valley and sea fog, lake
/// mist, steam after rain), and plain mist while nothing falls or blows. Rain, snow and
/// dust bring their own murk.
fn mist(shown: &Weather) -> f32 {
    let clear_air = (1.0 - shown.precipitation * 4.0).clamp(0.0, 1.0) * (1.0 - shown.dust * 3.0).clamp(0.0, 1.0);
    shown.ground_fog.max(shown.fog * clear_air) * 0.5
}

/// The shader's motion code for diamond dust: crystals hanging in the air that glint for
/// an instant as they turn (the only sparkle; snowflakes are soft).
const GLINT: f32 = 5.0;

/// How many glinting ice crystals hang in the air: diamond dust forms where a halo shows
/// in bitter cold, under a clear sky.
fn glints(shown: &Weather) -> f32 {
    let cold = 1.0 - ((shown.temperature + 18.0) / 6.0).clamp(0.0, 1.0);
    (shown.halo * cold * (1.0 - shown.precipitation * 4.0)).clamp(0.0, 1.0)
}

/// How the shader moves an ambient particle set.
fn motion_code(motion: Motion) -> f32 {
    match motion {
        Motion::Drift => 0.0,
        Motion::Hover => 1.0,
        Motion::Fall => 2.0,
        Motion::Rise => 3.0,
        Motion::Blow => 4.0,
    }
}

/// How an ambient particle set moves through the air, in blocks per second.
fn motion_velocity(motion: Motion, wind: Vec3) -> Vec3 {
    match motion {
        Motion::Drift => wind * 0.35,
        Motion::Hover => wind * 0.05,
        Motion::Fall => wind * 0.5 - Vec3::Y * 0.8,
        Motion::Rise => wind * 0.2 + Vec3::Y * 0.4,
        Motion::Blow => wind,
    }
}

/// Advances the falling drops, refreshes the height map and lights the drops like the sky.
#[allow(clippy::too_many_arguments)]
fn update(
    time: Res<Time>,
    weather: Res<ClientWeather>,
    air: Res<ClientAtmosphere>,
    world: Res<ClientWorld>,
    mut map: ResMut<SkyMap>,
    camera: Query<(&GlobalTransform, &AmbientLight, &DistanceFog), With<Camera3d>>,
    mut drops: Query<(&Drops, &MeshMaterial3d<PrecipitationMaterial>, &mut Visibility)>,
    mut materials: ResMut<Assets<PrecipitationMaterial>>,
    mut fallen: Local<Fallen>,
) {
    let Ok((camera, ambient, fog)) = camera.single() else { return };
    let Some(handle) = drops.iter().next().map(|(_, material, _)| material.0.clone()) else { return };
    let shown = &weather.current;
    // Leaves keep dripping for a few minutes after the rain, as the ground dries.
    let rain = shown.precipitation * (1.0 - shown.snow);
    let drips = (rain * 1.5 + shown.wetness * 0.6 - 0.05).clamp(0.0, 1.0);
    let glints = glints(shown);
    let mist = mist(shown);
    // Rain and snow ground the glowing motes and wisps (ball lightning in a storm would be
    // a few orbs, which these are not): nothing glows in the rain.
    let dry = 1.0 - ((shown.precipitation - 0.02) / 0.18).clamp(0.0, 1.0);
    let motes = shown.motes.max(shown.wisps * 0.3) * dry;
    for (kind, _, mut visibility) in &mut drops {
        let shows = match kind {
            // The distant layer carries blowing dust as well as rain and snow.
            Drops::Falling => shown.precipitation > 0.01 || drips > 0.0 || shown.dust > 0.01,
            Drops::Dust => shown.dust > 0.01,
            Drops::Motes => motes > 0.0,
            // The land's sets fill the first slots, the weather's (glints, mist) the last.
            Drops::Ambient(set) => {
                let weather_sets = usize::from(glints > 0.01) + usize::from(mist > 0.01);
                let set = *set as usize;
                set < air.particles.len().min(AMBIENT_SETS) || set >= AMBIENT_SETS - weather_sets
            }
            Drops::Splashes => shown.precipitation > 0.01,
            Drops::Warm => true,
        };
        visibility.set_if_neq(if shows { Visibility::Inherited } else { Visibility::Hidden });
    }
    if shown.precipitation <= 0.01 && drips <= 0.0 && shown.dust <= 0.01 && shown.motes <= 0.01 && glints <= 0.01 && mist <= 0.01 && air.particles.is_empty() {
        // Nothing shows: only the warm drop is drawn, and the shader hides it too.
        let showing = |p: &PrecipitationParams| {
            p.rain.w > 0.0 || p.far.w > 0.0 || p.dust.w > 0.0 || p.motes.w > 0.0 || p.ambient.iter().step_by(3).any(|a| a.w > 0.0)
        };
        if materials.get(&handle).is_some_and(|m| showing(&m.params))
            && let Some(mut material) = materials.get_mut(&handle)
        {
            material.params.rain.w = 0.0;
            material.params.far.w = 0.0;
            material.params.dust.w = 0.0;
            material.params.motes.w = 0.0;
            material.params.ambient.iter_mut().step_by(3).for_each(|a| a.w = 0.0);
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
    // Streaks lie along the rain's motion as seen from the moving camera, so running into
    // the rain tilts it towards you.
    if let Some(last) = fallen.last_eye
        && dt > 0.0
    {
        let moved = ((eye - last) / dt).clamp_length_max(30.0);
        fallen.camera_velocity = fallen.camera_velocity.lerp(moved, (dt * 8.0).min(1.0));
    }
    fallen.last_eye = Some(eye);
    let seen = rain_velocity - fallen.camera_velocity;
    let fall = |at: &mut Vec3, velocity: Vec3| *at = (*at + velocity * dt).rem_euclid(BOX);
    fall(&mut fallen.rain, rain_velocity);
    // The distant layer falls like the rain, or like the snow when most is snow.
    // Blowing dust rides the wind; otherwise it falls like the snow or the rain.
    let far_velocity = if shown.dust > shown.precipitation {
        wind - Vec3::Y * 0.3
    } else if shown.snow > 0.5 {
        snow_velocity
    } else {
        rain_velocity
    };
    fallen.far = (fallen.far + far_velocity * dt).rem_euclid(BOX * 3.0);
    fallen.snow = (fallen.snow + snow_velocity * dt).rem_euclid(BOX * 0.6);
    // Fine drizzle drifts with the wind, sleet less so, and hail falls hard.
    fall(&mut fallen.drizzle, wind * 0.9 - Vec3::Y * DRIZZLE_SPEED);
    fall(&mut fallen.sleet, wind * 0.7 - Vec3::Y * SLEET_SPEED);
    fall(&mut fallen.hail, wind * 0.3 - Vec3::Y * HAIL_SPEED);
    // Dust rides the wind, settling slowly; motes drift and rise.
    fall(&mut fallen.dust, wind - Vec3::Y * 0.3);
    fall(&mut fallen.motes, wind * 0.15 + Vec3::Y * 0.4);
    // Each ambient set keeps its own drift while it shows.
    fallen.ambient.retain(|(name, _)| air.particles.iter().any(|p| &p.name == name));
    let mut sets = [Vec4::ZERO; 3 * AMBIENT_SETS];
    for (i, set) in air.particles.iter().take(AMBIENT_SETS).enumerate() {
        let at = match fallen.ambient.iter_mut().find(|(name, _)| *name == set.name) {
            Some((_, at)) => at,
            None => {
                fallen.ambient.push((set.name.clone(), Vec3::ZERO));
                &mut fallen.ambient.last_mut().unwrap().1
            }
        };
        *at = (*at + motion_velocity(set.motion, wind) * dt).rem_euclid(BOX);
        // Sets ask for a share of the most shown.
        sets[i * 3] = at.extend(set.density.min(1.0));
        sets[i * 3 + 1] = set.color.extend(if set.glow { 1.0 } else { 0.0 });
        sets[i * 3 + 2] = Vec4::new(set.size, motion_code(set.motion), 0.0, 0.0);
    }
    // The weather's own sets take the last slots, before the land's sparsest: diamond
    // dust settling slowly, and mist drifting with the wind below the fog's top.
    let mut slot = AMBIENT_SETS;
    fallen.glints = (fallen.glints + (wind * 0.1 - Vec3::Y * 0.05) * dt).rem_euclid(BOX);
    if glints > 0.01 {
        slot -= 1;
        sets[slot * 3] = fallen.glints.extend((glints * 1.4).min(1.0));
        sets[slot * 3 + 1] = Vec3::new(0.85, 0.93, 1.0).extend(1.0);
        sets[slot * 3 + 2] = Vec4::new(0.06, GLINT, 0.0, 0.0);
    }
    fallen.mist = (fallen.mist + wind * 0.25 * dt).rem_euclid(BOX);
    if mist > 0.01 {
        slot -= 1;
        // Ground fog lies below its top; plain mist fills the air.
        let ceiling = if shown.ground_fog >= shown.fog { shown.fog_top - eye.y } else { 1000.0 };
        sets[slot * 3] = fallen.mist.extend(mist.min(1.0));
        sets[slot * 3 + 1] = Vec3::new(0.34, 0.35, 0.37).extend(0.0);
        sets[slot * 3 + 2] = Vec4::new(2.6, MIST, ceiling, 0.0);
    }

    let Some(mut material) = materials.get_mut(&handle) else { return };
    let light = ambient.color.to_linear();
    let haze = fog.color.to_linear();
    material.params = PrecipitationParams {
        camera: eye.rem_euclid(BOX).extend(time.elapsed_secs_wrapped()),
        map: Vec4::new(eye.x - map.origin.x as f32, 0.0, eye.z - map.origin.y as f32, eye.y),
        rain: fallen.rain.extend(density(shown.precipitation)),
        snow: (fallen.snow - eye).rem_euclid(BOX * 0.6).extend(shown.snow),
        streak: seen.normalize().extend(seen.length() * 0.045),
        light: Vec4::new(light.red, light.green, light.blue, shown.precipitation.max(shown.dust)),
        haze: Vec4::new(haze.red, haze.green, haze.blue, 0.0),
        far: (fallen.far - eye).rem_euclid(BOX * 3.0).extend(drips),
        drizzle: fallen.drizzle.extend(shown.drizzle),
        sleet: fallen.sleet.extend(shown.sleet),
        hail: fallen.hail.extend(shown.hail),
        dust: fallen.dust.extend(density(shown.dust)),
        motes: fallen.motes.extend(motes),
        dust_color: Color::srgb_u8(shown.dust_color[0], shown.dust_color[1], shown.dust_color[2]).to_linear().to_vec4(),
        wind: wind.extend(0.0),
        ambient: sets,
        heights: map.packed(),
    };
}
