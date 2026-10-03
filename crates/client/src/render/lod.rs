//! Distant terrain, in the spirit of Distant Horizons.
//!
//! Beyond the real chunks, the world is drawn as nested square rings of tiles. Each ring
//! doubles the cell size of the one inside it (4, 8, 16, 32 blocks), so detail falls off
//! with distance while the triangle count per ring stays constant. Tiles are blocky
//! heightfields sampled from the generator's pure surface functions on worker threads,
//! so they exist before the server has generated anything there. A fragment shader hides
//! every part of a ring that finer detail already covers, so rings never overlap.

use std::sync::Arc;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, VertexFormat,
};
use bevy::shader::ShaderRef;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::{BlockId, BlockRegistry, Tint};
use voxmar_engine::coords::{CHUNK_SIZE, Face, WORLD_HEIGHT};
use voxmar_worldgen::{Overworld, SEA_LEVEL};

use crate::textures::Rgba;

/// Number of rings. Cell size doubles per ring, starting at `FIRST_CELL` blocks.
pub const LEVELS: usize = 4;
const FIRST_CELL: i32 = 4;
const CELLS: i32 = 32;
/// Tiles from a ring's centre tile to its edge, so each ring is `2 * RING + 1` tiles wide.
const RING: i32 = 6;
const MAX_IN_FLIGHT: usize = 12;
const TILES_PER_FRAME: usize = 6;

pub const ATTRIBUTE_LOD: MeshVertexAttribute = MeshVertexAttribute::new("Lod", 0x6c6f_6431, VertexFormat::Uint32x2);

pub struct LodPlugin;

impl Plugin for LodPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "lod.wgsl");
        app.add_plugins(MaterialPlugin::<LodMaterial>::default()).add_systems(Update, (request_tiles, apply_tiles).chain());
    }
}

/// Far edge of the outermost ring, in blocks from the camera.
pub fn view_distance(real_columns: i32) -> f32 {
    let outer_tile = (FIRST_CELL << (LEVELS - 1)) * CELLS;
    ((RING as f32 + 0.5) * outer_tile as f32).max(real_columns as f32 * CHUNK_SIZE as f32)
}

#[derive(ShaderType, Clone, Copy, Default, Debug)]
pub struct LodParams {
    pub rings: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct LodMaterial {
    #[uniform(0)]
    pub params: LodParams,
}

impl Material for LodMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://voxmar/render/lod.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/render/lod.wgsl".into()
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
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[ATTRIBUTE_LOD.at_shader_location(0)])?];
        Ok(())
    }
}

/// Average colours of block faces, for colouring coarse cells.
pub struct LodPalette {
    top: Vec<[f32; 3]>,
    side: Vec<[f32; 3]>,
    tints: Vec<(Tint, Tint)>,
    grass: Rgba,
    foliage: Rgba,
}

impl LodPalette {
    pub fn new(registry: &BlockRegistry, textures: &[Rgba], colormaps: &[Rgba; 2]) -> Self {
        let average = |layer: u16| {
            let texture = &textures[layer as usize];
            let (mut sum, mut weight) = ([0.0f32; 3], 0.0f32);
            for pixel in texture.pixels.as_chunks::<4>().0 {
                let a = pixel[3] as f32 / 255.0;
                for c in 0..3 {
                    sum[c] += srgb_to_linear(pixel[c]) * a;
                }
                weight += a;
            }
            sum.map(|s| s / weight.max(1e-3))
        };
        let ids = (0..registry.len()).map(|i| registry.info(BlockId(i as u16)));
        let (mut top, mut side, mut tints) = (Vec::new(), Vec::new(), Vec::new());
        for info in ids {
            top.push(average(info.layers[Face::PosY as usize]));
            side.push(average(info.layers[Face::PosX as usize]));
            tints.push((info.tints[Face::PosY as usize], info.tints[Face::PosX as usize]));
        }
        Self { top, side, tints, grass: colormaps[0].clone(), foliage: colormaps[1].clone() }
    }

    fn color(&self, block: BlockId, top: bool, climate: [u8; 2]) -> [f32; 3] {
        let i = block.0 as usize;
        let (base, tint) = if top { (self.top[i], self.tints[i].0) } else { (self.side[i], self.tints[i].1) };
        let tint = match tint {
            Tint::None => [1.0; 3],
            Tint::Grass => colormap(&self.grass, climate),
            Tint::Foliage => colormap(&self.foliage, climate),
            Tint::Water => {
                let t = climate[0] as f32 / 255.0;
                [0.16 + (0.14 - 0.16) * t, 0.30 + (0.50 - 0.30) * t, 0.78 + (0.66 - 0.78) * t]
            }
        };
        [base[0] * tint[0], base[1] * tint[1], base[2] * tint[2]]
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb_byte(c: f32) -> u32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u32
}

/// Minecraft-style colormap lookup by temperature and humidity.
fn colormap(map: &Rgba, [temperature, humidity]: [u8; 2]) -> [f32; 3] {
    let t = temperature as f32 / 255.0;
    let h = humidity as f32 / 255.0 * t;
    let x = ((1.0 - t) * (map.width - 1) as f32) as u32;
    let y = ((1.0 - h) * (map.height - 1) as f32) as u32;
    let i = ((y * map.width + x) * 4) as usize;
    [srgb_to_linear(map.pixels[i]), srgb_to_linear(map.pixels[i + 1]), srgb_to_linear(map.pixels[i + 2])]
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct TileKey {
    level: u8,
    x: i32,
    z: i32,
}

impl TileKey {
    fn cell(self) -> i32 {
        FIRST_CELL << self.level
    }

    fn origin(self) -> IVec2 {
        IVec2::new(self.x, self.z) * self.cell() * CELLS
    }
}

struct TileResult {
    key: TileKey,
    mesh: Option<Mesh>,
}

/// Distant terrain state. Inactive until the server tells us the world seed.
#[derive(Resource)]
pub struct Lod {
    generator: Option<Arc<Overworld>>,
    palette: Arc<LodPalette>,
    materials: Vec<Handle<LodMaterial>>,
    real_columns: i32,
    tiles: FxHashMap<TileKey, Entity>,
    pending: FxHashSet<TileKey>,
    wanted: Vec<TileKey>,
    last_camera: Option<IVec2>,
    results_tx: Sender<TileResult>,
    results_rx: Receiver<TileResult>,
}

impl Lod {
    pub fn new(palette: LodPalette, real_columns: i32, materials: &mut Assets<LodMaterial>) -> Self {
        let materials = (0..LEVELS)
            .map(|level| {
                let cell = (FIRST_CELL << level) as f32;
                let finer = if level == 0 { 0.0 } else { cell / 2.0 };
                materials.add(LodMaterial { params: LodParams { rings: Vec4::new(cell, finer, RING as f32, real_columns as f32) } })
            })
            .collect();
        let (results_tx, results_rx) = unbounded();
        Self {
            generator: None,
            palette: Arc::new(palette),
            materials,
            real_columns,
            tiles: FxHashMap::default(),
            pending: FxHashSet::default(),
            wanted: Vec::new(),
            last_camera: None,
            results_tx,
            results_rx,
        }
    }

    /// Starts generating distant terrain for the world with this seed.
    pub fn start(&mut self, seed: u64, registry: Arc<BlockRegistry>) {
        self.generator = Some(Arc::new(Overworld::new(seed, registry)));
        self.last_camera = None;
    }

    /// Tiles shown and tiles wanted.
    pub fn progress(&self) -> (usize, usize) {
        (self.tiles.len(), self.wanted.len())
    }

    /// Tiles each ring needs around `camera`, nearest rings first: those in the ring's
    /// square that are not entirely covered by finer detail.
    fn wanted_tiles(&self, camera: IVec2) -> Vec<TileKey> {
        let mut wanted = Vec::new();
        for level in 0..LEVELS as u8 {
            let tile = (FIRST_CELL << level) * CELLS;
            let center = camera.div_euclid(IVec2::splat(tile));
            let (hole_low, hole_high) = if level == 0 {
                let column = camera.div_euclid(IVec2::splat(CHUNK_SIZE));
                ((column - (self.real_columns - 1)) * CHUNK_SIZE, (column + self.real_columns) * CHUNK_SIZE)
            } else {
                let finer = tile / 2;
                let c = camera.div_euclid(IVec2::splat(finer));
                ((c - RING) * finer, (c + RING + 1) * finer)
            };
            let mut ring: Vec<TileKey> = Vec::new();
            for z in center.y - RING..=center.y + RING {
                for x in center.x - RING..=center.x + RING {
                    let low = IVec2::new(x, z) * tile;
                    let high = low + tile;
                    let hidden = low.cmpge(hole_low).all() && high.cmple(hole_high).all();
                    if !hidden {
                        ring.push(TileKey { level, x, z });
                    }
                }
            }
            ring.sort_by_key(|k| (IVec2::new(k.x, k.z) - center).abs().max_element());
            wanted.extend(ring);
        }
        wanted
    }
}

fn request_tiles(mut commands: Commands, mut lod: ResMut<Lod>, camera: Query<&GlobalTransform, With<Camera3d>>) {
    let Some(generator) = lod.generator.clone() else { return };
    let Ok(camera) = camera.single() else { return };
    let position = camera.translation().xz().floor().as_ivec2();
    let column = position.div_euclid(IVec2::splat(CHUNK_SIZE));
    if lod.last_camera != Some(column) {
        lod.last_camera = Some(column);
        lod.wanted = lod.wanted_tiles(position);
        let keep: FxHashSet<TileKey> = lod.wanted.iter().copied().collect();
        lod.tiles.retain(|key, entity| {
            let keep = keep.contains(key);
            if !keep {
                commands.entity(*entity).despawn();
            }
            keep
        });
    }

    let lod = &mut *lod;
    for i in 0..lod.wanted.len() {
        if lod.pending.len() >= MAX_IN_FLIGHT {
            break;
        }
        let key = lod.wanted[i];
        if lod.tiles.contains_key(&key) || !lod.pending.insert(key) {
            continue;
        }
        let (generator, palette, tx) = (generator.clone(), lod.palette.clone(), lod.results_tx.clone());
        rayon::spawn(move || {
            let _ = tx.send(TileResult { key, mesh: build_tile(&generator, &palette, key) });
        });
    }
}

fn apply_tiles(mut commands: Commands, mut lod: ResMut<Lod>, mut meshes: ResMut<Assets<Mesh>>) {
    for _ in 0..TILES_PER_FRAME {
        let Ok(result) = lod.results_rx.try_recv() else { break };
        lod.pending.remove(&result.key);
        if !lod.wanted.contains(&result.key) {
            continue;
        }
        let Some(mesh) = result.mesh else { continue };
        let key = result.key;
        let cell = key.cell() as f32;
        let origin = key.origin();
        let entity = commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(lod.materials[key.level as usize].clone()),
                Transform::from_xyz(origin.x as f32, 0.0, origin.y as f32).with_scale(Vec3::new(cell, 1.0, cell)),
                Aabb::from_min_max(Vec3::ZERO, Vec3::new(CELLS as f32, WORLD_HEIGHT as f32, CELLS as f32)),
                NotShadowCaster,
            ))
            .id();
        if let Some(old) = lod.tiles.insert(key, entity) {
            commands.entity(old).despawn();
        }
    }
}

/// One coarse cell of a tile.
#[derive(Clone, Copy)]
struct Cell {
    /// World y of the top surface.
    top: i32,
    color: u32,
    side_color: u32,
    water: bool,
}

/// Samples the generator on the tile's grid (plus a border for side faces) and builds a
/// blocky heightfield mesh, merging equal neighbouring tops into runs.
fn build_tile(generator: &Overworld, palette: &LodPalette, key: TileKey) -> Option<Mesh> {
    let _span = info_span!("lod_tile").entered();
    let cell = key.cell();
    let origin = key.origin();
    const W: i32 = CELLS + 2;
    // Sample at cell centres.
    let heights: Vec<i32> = (0..W * W)
        .map(|i| {
            let (x, z) = (i % W - 1, i / W - 1);
            generator.height(origin.x + x * cell + cell / 2, origin.y + z * cell + cell / 2)
        })
        .collect();
    let height_at = |x: i32, z: i32| heights[((z + 1) * W + x + 1) as usize];

    let pack = |c: [f32; 3]| linear_to_srgb_byte(c[0]) | linear_to_srgb_byte(c[1]) << 8 | linear_to_srgb_byte(c[2]) << 16;
    let mut cells = Vec::with_capacity((W * W) as usize);
    for z in -1..=CELLS {
        for x in -1..=CELLS {
            let (wx, wz) = (origin.x + x * cell + cell / 2, origin.y + z * cell + cell / 2);
            let h = height_at(x, z);
            let slope = if (0..CELLS).contains(&x) && (0..CELLS).contains(&z) {
                [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().map(|(dx, dz)| (height_at(x + dx, z + dz) - h).abs()).max().unwrap_or(0) / cell
            } else {
                0
            };
            let surface = generator.surface_sample(wx, wz, h, slope);
            let mut c = if let Some(water) = surface.water {
                // Imitate looking through translucent water: the floor shows in the
                // shallows, deep water is dark.
                let depth = ((SEA_LEVEL - h) as f32 / 14.0).clamp(0.0, 1.0);
                let floor = palette.color(surface.ground, true, surface.climate);
                let water = palette.color(water, true, surface.climate);
                let mixed = std::array::from_fn(|i| floor[i] * water[i] * 2.0 * (1.0 - depth) + water[i] * 0.45 * depth);
                let color = pack(mixed);
                Cell { top: SEA_LEVEL + 1, color, side_color: color, water: true }
            } else {
                Cell {
                    top: h + 1,
                    color: pack(palette.color(surface.ground, true, surface.climate)),
                    side_color: pack(palette.color(surface.ground, false, surface.climate)),
                    water: false,
                }
            };
            // Tree cover: raise a share of cells by a crown, coloured like the leaves.
            if let Some((leaves, cover)) = surface.canopy {
                let hash = (wx as u32).wrapping_mul(0x9e37_79b1) ^ (wz as u32).wrapping_mul(0x85eb_ca77);
                let roll = (hash.wrapping_mul(0xc2b2_ae3d) >> 8) as f32 / (1u32 << 24) as f32;
                if roll < cover {
                    c.top += 4 + (hash >> 29) as i32;
                    c.color = pack(palette.color(leaves, true, surface.climate));
                    c.side_color = pack(palette.color(leaves, false, surface.climate).map(|v| v * 0.8));
                }
            }
            cells.push(c);
        }
    }
    let at = |x: i32, z: i32| cells[((z + 1) * W + x + 1) as usize];

    let mut vertices: Vec<[u32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut quad = |corners: [(i32, i32, i32); 4], face: Face, color: u32, water: bool| {
        let base = vertices.len() as u32;
        for (x, y, z) in corners {
            let w0 = x as u32 | (z as u32) << 6 | (y.clamp(0, 511) as u32) << 12 | (face as u32) << 21 | (water as u32) << 24;
            vertices.push([w0, color]);
        }
        indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
    };

    for z in 0..CELLS {
        // Tops, merged into runs along x.
        let mut x = 0;
        while x < CELLS {
            let c = at(x, z);
            let mut end = x + 1;
            while end < CELLS && {
                let n = at(end, z);
                n.top == c.top && n.color == c.color && n.water == c.water
            } {
                end += 1;
            }
            quad([(x, c.top, z), (x, c.top, z + 1), (end, c.top, z + 1), (end, c.top, z)], Face::PosY, c.color, c.water);
            x = end;
        }
        // Sides wherever a neighbour is lower.
        for x in 0..CELLS {
            let c = at(x, z);
            let sides = [
                (at(x + 1, z).top, Face::PosX, [(x + 1, 0, z + 1), (x + 1, 0, z), (x + 1, 1, z), (x + 1, 1, z + 1)]),
                (at(x - 1, z).top, Face::NegX, [(x, 0, z), (x, 0, z + 1), (x, 1, z + 1), (x, 1, z)]),
                (at(x, z + 1).top, Face::PosZ, [(x, 0, z + 1), (x + 1, 0, z + 1), (x + 1, 1, z + 1), (x, 1, z + 1)]),
                (at(x, z - 1).top, Face::NegZ, [(x + 1, 0, z), (x, 0, z), (x, 1, z), (x + 1, 1, z)]),
            ];
            for (neighbour, face, corners) in sides {
                if neighbour >= c.top {
                    continue;
                }
                // Corner y of 0 means the neighbour's top, 1 means ours.
                let corners = corners.map(|(cx, level, cz)| (cx, if level == 0 { neighbour } else { c.top }, cz));
                quad(corners, face, c.side_color, false);
            }
        }
    }
    if indices.is_empty() {
        return None;
    }
    let indices = if vertices.len() <= u16::MAX as usize + 1 {
        Indices::U16(indices.into_iter().map(|i| i as u16).collect())
    } else {
        Indices::U32(indices)
    };
    Some(
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(ATTRIBUTE_LOD, VertexAttributeValues::Uint32x2(vertices))
            .with_inserted_indices(indices),
    )
}
