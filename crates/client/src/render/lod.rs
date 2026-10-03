//! Distant terrain, in the spirit of Distant Horizons.
//!
//! Beyond the real chunks, the world is drawn as nested square rings of tiles. Each ring
//! doubles the cell size of the one inside it (4, 8, 16, 32 blocks), so detail falls off
//! with distance while the triangle count per ring stays constant. Tiles are smooth
//! heightfields sampled from the generator's pure surface functions on worker threads,
//! so they exist before the server has generated anything there. A fragment shader hides
//! every part of a ring that finer detail already covers, so rings never overlap.
//!
//! The heightfield is the true ground, sea floor included, with tree crowns raised over
//! forests. Water and ice are a separate sheet at sea level over it, so the floor shows
//! through real translucent water at the edge of the real chunks. Vertices carry the
//! albedo of top and side faces; the shader shades the heightfield as the staircase of
//! blocks it stands for (see `lod.wgsl`), with the same lighting as real chunks.

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

pub const ATTRIBUTE_LOD: MeshVertexAttribute = MeshVertexAttribute::new("Lod", 0x6c6f_6433, VertexFormat::Uint32x4);

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
    /// Average opacity of top faces, for seeing through ice.
    opacity: Vec<f32>,
    tints: Vec<(Tint, Tint)>,
    grass: Rgba,
    foliage: Rgba,
}

impl LodPalette {
    pub fn new(registry: &BlockRegistry, textures: &[Rgba], colormaps: &[Rgba; 2]) -> Self {
        let average = |layer: u16| {
            let texture = &textures[layer as usize];
            // Average in sRGB space, as the texture mipmaps do, so distant terrain has the
            // same colour as far-away real chunks.
            let (mut sum, mut weight) = ([0.0f32; 3], 0.0f32);
            let pixels = texture.pixels.as_chunks::<4>().0;
            for pixel in pixels {
                let a = pixel[3] as f32 / 255.0;
                for c in 0..3 {
                    sum[c] += pixel[c] as f32 * a;
                }
                weight += a;
            }
            (sum.map(|s| srgb_to_linear((s / weight.max(1e-3)).round() as u8)), weight / pixels.len().max(1) as f32)
        };
        let ids = (0..registry.len()).map(|i| registry.info(BlockId(i as u16)));
        let (mut top, mut side, mut opacity, mut tints) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for info in ids {
            let (color, alpha) = average(info.layers[Face::PosY as usize]);
            top.push(color);
            opacity.push(alpha);
            side.push(average(info.layers[Face::PosX as usize]).0);
            tints.push((info.tints[Face::PosY as usize], info.tints[Face::PosX as usize]));
        }
        Self { top, side, opacity, tints, grass: colormaps[0].clone(), foliage: colormaps[1].clone() }
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

    /// Top and side colours of a block.
    fn faces(&self, block: BlockId, climate: [u8; 2]) -> ([f32; 3], [f32; 3]) {
        (self.color(block, true, climate), self.color(block, false, climate))
    }

    /// Water gets ripples and reflections; other see-through blocks only their opacity.
    fn is_water(&self, block: BlockId) -> bool {
        self.tints[block.0 as usize].0 == Tint::Water
    }

    /// Grass grows plants: short and tall grass, ferns and flowers.
    fn is_grassy(&self, block: BlockId) -> bool {
        self.tints[block.0 as usize].0 == Tint::Grass
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

    /// Removes all distant terrain and stops making it until `start` is called again.
    /// Tiles still being built are discarded.
    pub fn clear(&mut self, commands: &mut Commands, materials: &mut Assets<LodMaterial>) {
        for (_, entity) in self.tiles.drain() {
            commands.entity(entity).despawn();
        }
        self.generator = None;
        self.pending.clear();
        self.wanted.clear();
        (self.results_tx, self.results_rx) = unbounded();
        self.set_real_columns(0, materials);
    }

    /// Radius, in columns, of the real chunks the rings surround.
    pub fn real_columns(&self) -> i32 {
        self.real_columns
    }

    pub fn set_real_columns(&mut self, real_columns: i32, materials: &mut Assets<LodMaterial>) {
        self.real_columns = real_columns;
        for handle in &self.materials {
            if let Some(mut material) = materials.get_mut(handle) {
                material.params.rings.w = real_columns as f32;
            }
        }
        // Recompute the wanted tiles.
        self.last_camera = None;
    }

    /// Tiles shown and tiles wanted.
    pub fn progress(&self) -> (usize, usize) {
        (self.tiles.len(), self.wanted.len())
    }

    /// Tiles each ring needs around `camera`, nearest rings first: those in the ring's
    /// square that are not entirely covered by finer detail.
    fn wanted_tiles(&self, camera: IVec2) -> Vec<TileKey> {
        let column = camera.div_euclid(IVec2::splat(CHUNK_SIZE));
        let real = ((column - (self.real_columns - 1)) * CHUNK_SIZE, (column + self.real_columns) * CHUNK_SIZE);
        let mut wanted = Vec::new();
        for level in 0..LEVELS as u8 {
            let tile = (FIRST_CELL << level) * CELLS;
            let center = camera.div_euclid(IVec2::splat(tile));
            let finer = if level == 0 {
                real
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
                    // Real chunks can reach past the finer rings at long view distances.
                    let inside = |(hole_low, hole_high): (IVec2, IVec2)| low.cmpge(hole_low).all() && high.cmple(hole_high).all();
                    let hidden = inside(finer) || inside(real);
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

/// What the terrain looks like at a grid corner of a tile.
#[derive(Clone, Copy)]
struct Corner {
    /// Top of the terrain: dry ground, the sea floor, or tree crowns over forests.
    height: f32,
    /// Linear albedo of top and side faces.
    top: [f32; 3],
    side: [f32; 3],
    /// Sky light at the terrain surface, 0..=15. It fades with depth under water.
    sky: u8,
    /// Fraction of the ground under tree crowns.
    canopy: f32,
    /// Fraction of the ground with a small plant on it.
    plants: f32,
    /// Water or ice at sea level above the terrain.
    sheet: Option<BlockId>,
    climate: [u8; 2],
}

/// Seen from afar, leaves are darker than their texture: the gaps show the shaded inside of
/// the crown.
const CROWN_SHADE: f32 = 0.8;
/// How far crowns of a dense forest raise the canopy above the ground.
const CANOPY_HEIGHT: f32 = 6.0;
/// Fraction of open grassland with grass, ferns or flowers growing on it, about what the
/// generator plants.
const GRASS_PLANTS: f32 = 0.4;
/// Depth of the walls hanging from tile edges, hiding cracks between rings.
const SKIRT: f32 = 24.0;
/// Water and ice sheets lie a hair below the top of real water, so dry ground at sea level
/// stays above them.
const SHEET_HEIGHT: f32 = (SEA_LEVEL + 1) as f32 - 1.0 / 32.0;
/// Vertex kinds, see `lod.wgsl`.
const TERRAIN: u32 = 0;
const WATER: u32 = 1;
const ICE: u32 = 2;

/// Samples the generator at the tile's grid corners and builds a smooth heightfield mesh of
/// the ground, plus a flat sheet over cells with water or ice.
fn build_tile(generator: &Overworld, palette: &LodPalette, key: TileKey) -> Option<Mesh> {
    let _span = info_span!("lod_tile").entered();
    let cell = key.cell();
    let origin = key.origin();
    // Corners 0..=CELLS plus one more on each side for normals.
    const W: i32 = CELLS + 3;
    let at = |i: i32| (i - 1) * cell;
    let heights: Vec<i32> =
        (0..W * W).map(|i| generator.height(origin.x + at(i % W), origin.y + at(i / W))).collect();
    let height = |x: i32, z: i32| heights[((z + 1) * W + x + 1) as usize];
    // The regional climate is shared by every sample of the tile.
    let sampler = generator.sampler(origin - cell, origin + (CELLS + 1) * cell);

    let mut corners = Vec::with_capacity((W * W) as usize);
    for z in -1..=CELLS + 1 {
        for x in -1..=CELLS + 1 {
            let (wx, wz) = (origin.x + x * cell, origin.y + z * cell);
            let h = height(x, z);
            // Steepness per block, rounded like the whole steps between real columns.
            let slope = if (0..=CELLS).contains(&x) && (0..=CELLS).contains(&z) {
                let rise = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().map(|(dx, dz)| (height(x + dx, z + dz) - h).abs()).max().unwrap_or(0);
                (rise + cell / 2) / cell
            } else {
                0
            };
            let surface = sampler.surface(wx, wz, h, slope);
            let (top, side) = palette.faces(surface.ground, surface.climate);
            let (leaves, cover) = surface.canopy.unwrap_or((surface.ground, 0.0));
            let (crown_top, crown_side) = palette.faces(leaves, surface.climate);
            let crown = |ground: [f32; 3], leaves: [f32; 3]| {
                std::array::from_fn(|i| ground[i] + (leaves[i] * CROWN_SHADE - ground[i]) * cover)
            };
            corners.push(Corner {
                height: h as f32 + 1.0 + CANOPY_HEIGHT * cover,
                top: crown(top, crown_top),
                side: crown(side, crown_side),
                // Sky light fades by two levels per block of water or ice.
                sky: (15 - 2 * (SEA_LEVEL - h).max(0)).max(0) as u8,
                canopy: cover,
                plants: if palette.is_grassy(surface.ground) { GRASS_PLANTS * (1.0 - cover) } else { 0.0 },
                sheet: surface.water,
                climate: surface.climate,
            });
        }
    }
    let corner = |x: i32, z: i32| corners[((z + 1) * W + x + 1) as usize];

    let pack_color = |c: [f32; 3]| linear_to_srgb_byte(c[0]) | linear_to_srgb_byte(c[1]) << 8 | linear_to_srgb_byte(c[2]) << 16;
    let pack_position = |x: i32, z: i32, height: f32, sky: u8, kind: u32| {
        let y = (height * 32.0).round().clamp(0.0, 16383.0) as u32;
        x as u32 | (z as u32) << 6 | y << 12 | (sky as u32) << 26 | kind << 30
    };
    let terrain_vertex = |x: i32, z: i32, height: f32, normal: Vec3, c: Corner| {
        [
            pack_position(x, z, height, c.sky, TERRAIN),
            pack_color(c.top) | ((c.canopy * 255.0) as u32) << 24,
            encode_normal(normal),
            pack_color(c.side) | ((c.plants * 255.0) as u32) << 24,
        ]
    };
    let mut vertices: Vec<[u32; 4]> = Vec::with_capacity(((CELLS + 1) * (CELLS + 1) + 4 * (CELLS + 1)) as usize);
    for z in 0..=CELLS {
        for x in 0..=CELLS {
            let c = corner(x, z);
            let dx = corner(x + 1, z).height - corner(x - 1, z).height;
            let dz = corner(x, z + 1).height - corner(x, z - 1).height;
            let normal = Vec3::new(-dx, 2.0 * cell as f32, -dz).normalize();
            vertices.push(terrain_vertex(x, z, c.height, normal, c));
        }
    }
    let index = |x: i32, z: i32| (z * (CELLS + 1) + x) as u32;
    let mut indices = Vec::with_capacity((CELLS * CELLS * 6 + 4 * CELLS * 6) as usize);
    for z in 0..CELLS {
        for x in 0..CELLS {
            let (a, b, c, d) = (index(x, z), index(x + 1, z), index(x + 1, z + 1), index(x, z + 1));
            indices.extend([a, d, c, a, c, b]);
        }
    }
    // Skirts: each border edge gets a wall hanging down, facing outwards, which closes the
    // crack to whatever lies beyond the edge, on the side nearer the camera.
    // (start corner, step along the edge, outward normal, winding flipped)
    let borders = [
        ((0, 0), (1, 0), Vec3::NEG_Z, true),
        ((0, CELLS), (1, 0), Vec3::Z, false),
        ((0, 0), (0, 1), Vec3::NEG_X, false),
        ((CELLS, 0), (0, 1), Vec3::X, true),
    ];
    for ((sx, sz), (dx, dz), outward, flip) in borders {
        let edge = |i: i32| (sx + dx * i, sz + dz * i);
        let base = vertices.len() as u32;
        for i in 0..=CELLS {
            let (x, z) = edge(i);
            let c = corner(x, z);
            vertices.push(terrain_vertex(x, z, c.height - SKIRT, outward, c));
        }
        for i in 0..CELLS {
            let (x0, z0) = edge(i);
            let (x1, z1) = edge(i + 1);
            let (top0, top1, bottom0, bottom1) = (index(x0, z0), index(x1, z1), base + i as u32, base + i as u32 + 1);
            if flip {
                indices.extend([top0, top1, bottom1, top0, bottom1, bottom0]);
            } else {
                indices.extend([top0, bottom1, top1, top0, bottom0, bottom1]);
            }
        }
    }

    // The sheet covers every cell with water or ice at a corner; dry ground pokes through
    // it along the shore. Its vertices carry the floor beneath as their side colour.
    let mut sheet = vec![u32::MAX; ((CELLS + 1) * (CELLS + 1)) as usize];
    for z in 0..CELLS {
        for x in 0..CELLS {
            let cell_corners = [(x, z), (x + 1, z), (x + 1, z + 1), (x, z + 1)];
            let Some(block) = cell_corners.iter().find_map(|&(x, z)| corner(x, z).sheet) else { continue };
            for (x, z) in cell_corners {
                let slot = &mut sheet[index(x, z) as usize];
                if *slot == u32::MAX {
                    *slot = vertices.len() as u32;
                    let c = corner(x, z);
                    let block = c.sheet.unwrap_or(block);
                    let (kind, opacity) =
                        if palette.is_water(block) { (WATER, 1.0) } else { (ICE, palette.opacity[block.0 as usize]) };
                    vertices.push([
                        pack_position(x, z, SHEET_HEIGHT, c.sky, kind),
                        pack_color(palette.color(block, true, c.climate)) | ((opacity * 255.0) as u32) << 24,
                        encode_normal(Vec3::Y),
                        pack_color(c.top),
                    ]);
                }
            }
            let [a, b, c, d] = cell_corners.map(|(x, z)| sheet[index(x, z) as usize]);
            indices.extend([a, d, c, a, c, b]);
        }
    }

    let indices = if vertices.len() <= u16::MAX as usize + 1 {
        Indices::U16(indices.into_iter().map(|i| i as u16).collect())
    } else {
        Indices::U32(indices)
    };
    Some(
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(ATTRIBUTE_LOD, VertexAttributeValues::Uint32x4(vertices))
            .with_inserted_indices(indices),
    )
}

/// Octahedral encoding of a unit normal into two 8-bit components.
fn encode_normal(n: Vec3) -> u32 {
    let n = n / (n.x.abs() + n.y.abs() + n.z.abs());
    let (mut x, mut z) = (n.x, n.z);
    if n.y < 0.0 {
        (x, z) = ((1.0 - n.z.abs()) * n.x.signum(), (1.0 - n.x.abs()) * n.z.signum());
    }
    let quantize = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u32;
    quantize(x) | quantize(z) << 8
}
