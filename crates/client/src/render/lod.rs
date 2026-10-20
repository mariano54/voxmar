//! Distant terrain, in the spirit of Distant Horizons.
//!
//! Beyond the real chunks, the world is drawn as nested square rings of tiles. Each ring
//! doubles the cell size of the one inside it (4, 8, 16, 32 blocks), so detail falls off
//! with distance while the triangle count per ring stays constant. Tiles are smooth
//! heightfields sampled from the generator's pure surface functions on worker threads,
//! so they exist before the server has generated anything there. A fragment shader hides
//! every part of a ring that finer detail already covers: the next finer ring, or real
//! chunks wherever they have actually been meshed, so loading never leaves a hole. Tiles
//! are drawn by the terrain renderer (`terrain.rs`), not as entities.
//!
//! The heightfield is the true ground, sea and lake floors and river beds included, with
//! tree crowns raised over forests. Water and ice are a separate sheet over it at the level
//! of their water (the sea, or a lake's or river's own), so the floor shows through real
//! translucent water at the edge of the real chunks. Colours are the biome's (grass,
//! foliage and water) from the generator's surface samples. Vertices carry the
//! albedo of the ground's top and side faces and of tree crowns, and how much of the
//! ground is under crowns or small plants; the shader shades the heightfield as the
//! staircase of blocks it stands for (see `lod.wgsl`), with the same lighting as real
//! chunks.

use std::ops::Range;
use std::sync::Arc;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::FxHashSet;
use voxmar_engine::block::{BlockId, BlockRegistry, Tint};
use voxmar_engine::content::Content;
use voxmar_engine::coords::{CHUNK_SIZE, Face};
use voxmar_worldgen::{BiomeColors, Overworld, WorldContent};

use super::LayerInfo;
use super::terrain::Terrain;
use crate::textures::Rgba;

/// Number of rings. Cell size doubles per ring, starting at `FIRST_CELL` blocks.
pub const LEVELS: usize = 4;
pub const FIRST_CELL: i32 = 4;
pub const CELLS: i32 = 32;
/// Tiles from a ring's centre tile to its edge, so each ring is `2 * RING + 1` tiles wide.
const RING: i32 = 6;
const MAX_IN_FLIGHT: usize = 12;
const TILES_PER_FRAME: usize = 6;

/// Vertex of a tile: see `lod.wgsl`.
pub type LodVertex = [u32; 4];

pub struct LodPlugin;

impl Plugin for LodPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (request_tiles, apply_tiles).chain());
    }
}

/// Far edge of the outermost ring, in blocks from the camera.
pub fn view_distance(real_columns: i32) -> f32 {
    let outer_tile = (FIRST_CELL << (LEVELS - 1)) * CELLS;
    ((RING as f32 + 0.5) * outer_tile as f32).max(real_columns as f32 * CHUNK_SIZE as f32)
}

/// Average colours of block faces, for colouring coarse cells.
#[derive(Clone)]
pub struct LodPalette {
    top: Vec<[f32; 3]>,
    side: Vec<[f32; 3]>,
    /// The side's tinted overlay (a grass block's fringe of grass): its average colour
    /// where it covers the side, how much of the side it covers, and its tint.
    overlay: Vec<Option<([f32; 3], f32, Tint)>>,
    /// Average opacity of top faces, for seeing through ice.
    opacity: Vec<f32>,
    tints: Vec<(Tint, Tint)>,
    /// For blocks the surface lays in bands by height (strata), the top and side colours of
    /// their bands blended, as a banded slope or cliff looks from afar (`with_strata`).
    banded: Vec<Option<([f32; 3], [f32; 3])>>,
}

impl LodPalette {
    pub fn new(registry: &BlockRegistry, textures: &[Rgba], layers: &[LayerInfo]) -> Self {
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
        let (mut top, mut side, mut overlay, mut opacity, mut tints) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for info in ids {
            let (color, alpha) = average(info.layers[Face::PosY as usize]);
            top.push(color);
            opacity.push(alpha);
            let layer = info.layers[Face::PosX as usize];
            side.push(average(layer).0);
            overlay.push(layers.get(layer as usize).and_then(|l| l.overlay).map(|(over, tint)| {
                let (color, cover) = average(over);
                (color, cover, tint)
            }));
            tints.push((info.tints[Face::PosY as usize], info.tints[Face::PosX as usize]));
        }
        let banded = vec![None; top.len()];
        Self { top, side, overlay, opacity, tints, banded }
    }

    /// This palette with the blocks of each of `strata` coloured as their bands blended. A
    /// coarse cell's corner samples the surface at one height, so it took whichever band lay
    /// at that height, and banded cliffs were striped in every colour of their bands.
    pub fn with_strata<'a>(&self, strata: impl Iterator<Item = &'a [BlockId]>) -> Self {
        let mut sums = vec![([0.0f32; 3], [0.0f32; 3], 0.0f32); self.top.len()];
        for bands in strata {
            let n = bands.len() as f32;
            let blend = |faces: &[[f32; 3]]| {
                bands.iter().fold([0.0; 3], |sum, band| std::array::from_fn::<f32, 3, _>(|c| sum[c] + faces[band.0 as usize][c] / n))
            };
            let (top, side) = (blend(&self.top), blend(&self.side));
            for band in bands {
                let sum = &mut sums[band.0 as usize];
                sum.0 = std::array::from_fn(|c| sum.0[c] + top[c]);
                sum.1 = std::array::from_fn(|c| sum.1[c] + side[c]);
                sum.2 += 1.0;
            }
        }
        let banded = sums.into_iter().map(|(top, side, sets)| (sets > 0.0).then(|| (top.map(|v| v / sets), side.map(|v| v / sets)))).collect();
        Self { banded, ..self.clone() }
    }

    /// The colour of a block's top or side faces where the biome colours are `colors`.
    fn color(&self, block: BlockId, top: bool, colors: &BiomeColors) -> [f32; 3] {
        let i = block.0 as usize;
        let colour = |tint| match tint {
            Tint::None => [1.0; 3],
            Tint::Grass => colors.grass.map(srgb_to_linear),
            Tint::Foliage => colors.foliage.map(srgb_to_linear),
            Tint::Water => colors.water.map(srgb_to_linear),
        };
        let (base, tint) = if top { (self.top[i], self.tints[i].0) } else { (self.side[i], self.tints[i].1) };
        let tint = colour(tint);
        let mut color: [f32; 3] = std::array::from_fn(|c| base[c] * tint[c]);
        // An overlay covers part of the side: its colour, tinted, takes that part's place.
        if !top && let Some((over, cover, over_tint)) = self.overlay[i] {
            let tint = colour(over_tint);
            color = std::array::from_fn(|c| color[c] * (1.0 - cover) + over[c] * tint[c] * cover);
        }
        color
    }

    /// Top and side colours of a block, its bands' blend for one laid in bands.
    fn faces(&self, block: BlockId, colors: &BiomeColors) -> ([f32; 3], [f32; 3]) {
        if let Some(faces) = self.banded[block.0 as usize] {
            return faces;
        }
        (self.color(block, true, colors), self.color(block, false, colors))
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

/// A tile of one ring.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TileKey {
    pub level: u8,
    pub x: i32,
    pub z: i32,
}

impl TileKey {
    /// Cell size in blocks.
    pub fn cell(self) -> i32 {
        FIRST_CELL << self.level
    }

    pub fn size(self) -> i32 {
        self.cell() * CELLS
    }

    /// Corner of the tile, in blocks.
    pub fn origin(self) -> IVec2 {
        IVec2::new(self.x, self.z) * self.size()
    }
}

/// A built tile, ready for the terrain renderer.
pub struct TileMesh {
    pub key: TileKey,
    /// Lowest and highest block the tile reaches (skirts included), for culling.
    pub heights: Range<i32>,
    pub vertices: Vec<LodVertex>,
    pub indices: Vec<u32>,
}

/// The generator of the world on screen (the one being played, or the menus' backdrop),
/// made with the loaded content packages so that modded biomes, trees and plants show far
/// away as they do nearby. Distant terrain follows it, and anything else on the client
/// that samples the world beyond the real chunks (ambience) shares it rather than
/// building its own. None between worlds.
#[derive(Resource)]
pub struct WorldGenerator {
    content: Arc<WorldContent>,
    current: Option<Arc<Overworld>>,
}

impl WorldGenerator {
    /// What the worlds are made from: biomes, trees, plants.
    pub fn content(&self) -> &WorldContent {
        &self.content
    }

    pub fn new(content: Arc<WorldContent>) -> Self {
        Self { content, current: None }
    }

    /// Makes the generator of the world with this seed the current one, and returns it:
    /// the one the server in this process plays on (`Overworld::shared`), if it holds one.
    pub fn start(&mut self, seed: u64, content: &Content) -> Arc<Overworld> {
        let generator = Overworld::shared(seed, content, &self.content);
        self.current = Some(generator.clone());
        generator
    }

    /// Leaves the world: distant terrain is removed until the next `start`.
    pub fn clear(&mut self) {
        self.current = None;
    }

    pub fn get(&self) -> Option<&Arc<Overworld>> {
        self.current.as_ref()
    }
}

/// Distant terrain state. Follows [`WorldGenerator`]: empty until there is a world.
#[derive(Resource)]
pub struct Lod {
    /// The generator the tiles come from.
    generator: Option<Arc<Overworld>>,
    palette: Arc<LodPalette>,
    terrain: Terrain,
    real_columns: i32,
    tiles: FxHashSet<TileKey>,
    pending: FxHashSet<TileKey>,
    wanted: Vec<TileKey>,
    last_camera: Option<IVec2>,
    /// Where the rings are laid out around for drawing (the shader hides each ring inside
    /// the next finer one as laid out here), and the tiles that layout needs. It moves to
    /// the camera only once every tile wanted there has arrived, and those tiles stay
    /// until it does: rings laid out at once around a moving camera hid the coarser ring
    /// where the finer ring's new tiles were still being built, and the bare floor under
    /// the sea showed through as a long straight strip (flight shots, 2026-10-06).
    shown: FxHashSet<TileKey>,
    /// Whether the layout drawn is the one wanted now.
    settled: bool,
    /// The camera position the wanted tiles are laid out around.
    wanting: IVec2,
    results_tx: Sender<Option<TileMesh>>,
    results_rx: Receiver<Option<TileMesh>>,
}

impl Lod {
    pub fn new(palette: LodPalette, terrain: Terrain, real_columns: i32) -> Self {
        let (results_tx, results_rx) = unbounded();
        Self {
            generator: None,
            palette: Arc::new(palette),
            terrain,
            real_columns,
            tiles: FxHashSet::default(),
            pending: FxHashSet::default(),
            wanted: Vec::new(),
            last_camera: None,
            shown: FxHashSet::default(),
            settled: false,
            wanting: IVec2::ZERO,
            results_tx,
            results_rx,
        }
    }

    /// Removes all distant terrain and makes it anew from `generator`, if any, around a
    /// world with no real chunks yet. Tiles still being built are discarded.
    fn follow(&mut self, generator: Option<Arc<Overworld>>) {
        for key in self.tiles.drain() {
            self.terrain.remove_tile(key);
        }
        if let Some(generator) = &generator {
            self.palette = Arc::new(self.palette.with_strata(generator.strata()));
        }
        self.generator = generator;
        self.pending.clear();
        self.wanted.clear();
        self.shown.clear();
        self.settled = false;
        (self.results_tx, self.results_rx) = unbounded();
        self.set_real_columns(0);
    }

    /// Removes the tiles not in `keep`.
    fn drop_tiles_but(&mut self, keep: &FxHashSet<TileKey>) {
        let terrain = &self.terrain;
        self.tiles.retain(|key| {
            let kept = keep.contains(key);
            if !kept {
                terrain.remove_tile(*key);
            }
            kept
        });
    }

    /// Radius, in columns, of the real chunks the rings surround.
    pub fn real_columns(&self) -> i32 {
        self.real_columns
    }

    pub fn set_real_columns(&mut self, real_columns: i32) {
        self.real_columns = real_columns;
        self.terrain.set_real_columns(real_columns);
        // Recompute the wanted tiles.
        self.last_camera = None;
    }

    /// Tiles wanted that have arrived, and tiles wanted.
    pub fn progress(&self) -> (usize, usize) {
        (self.wanted.iter().filter(|key| self.tiles.contains(key)).count(), self.wanted.len())
    }

    /// Tiles each ring needs around `camera`, nearest rings first: those in the ring's
    /// square that are not entirely covered by the next finer ring. Tiles under the real
    /// chunks are kept, to stand in wherever those have not been meshed yet.
    fn wanted_tiles(&self, camera: IVec2) -> Vec<TileKey> {
        let mut wanted = Vec::new();
        for level in 0..LEVELS as u8 {
            let tile = (FIRST_CELL << level) * CELLS;
            let center = camera.div_euclid(IVec2::splat(tile));
            let finer = {
                let finer = tile / 2;
                let c = camera.div_euclid(IVec2::splat(finer));
                ((c - RING) * finer, (c + RING + 1) * finer)
            };
            let mut ring: Vec<TileKey> = Vec::new();
            for z in center.y - RING..=center.y + RING {
                for x in center.x - RING..=center.x + RING {
                    let low = IVec2::new(x, z) * tile;
                    let high = low + tile;
                    // Tiles where real chunks belong are kept all the same: those chunks arrive
                    // over many frames (a fast flight outruns them by hundreds of blocks),
                    // and the shader hides a tile only where they have. Hiding them here left
                    // a hole between the finest ring and the real chunks still to come, which
                    // showed as a long straight strip of sky and sunken sea floor.
                    let hidden = level > 0 && low.cmpge(finer.0).all() && high.cmple(finer.1).all();
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

fn request_tiles(mut lod: ResMut<Lod>, world: Res<WorldGenerator>, camera: Query<&GlobalTransform, With<Camera3d>>) {
    if !Option::eq(&lod.generator.as_ref().map(Arc::as_ptr), &world.get().map(Arc::as_ptr)) {
        lod.follow(world.get().cloned());
    }
    let Some(generator) = lod.generator.clone() else { return };
    let Ok(camera) = camera.single() else { return };
    let position = camera.translation().xz().floor().as_ivec2();
    let column = position.div_euclid(IVec2::splat(CHUNK_SIZE));
    let lod = &mut *lod;
    if lod.last_camera != Some(column) {
        lod.last_camera = Some(column);
        lod.wanted = lod.wanted_tiles(position);
        lod.settled = false;
        lod.wanting = position;
        let mut keep: FxHashSet<TileKey> = lod.wanted.iter().copied().collect();
        keep.extend(&lod.shown);
        lod.drop_tiles_but(&keep);
    }
    // The layout follows the camera once everything it wants is in, the first time too:
    // a layout settled before its tiles came would leave holes until it moved on, and its
    // tiles, no longer wanted, would never be asked for.
    if !lod.settled && lod.wanted.iter().all(|key| lod.tiles.contains(key)) {
        lod.settled = true;
        lod.terrain.set_rings_around(lod.wanting);
        lod.shown = lod.wanted.iter().copied().collect();
        let keep = std::mem::take(&mut lod.shown);
        lod.drop_tiles_but(&keep);
        lod.shown = keep;
    }

    for i in 0..lod.wanted.len() {
        if lod.pending.len() >= MAX_IN_FLIGHT {
            break;
        }
        let key = lod.wanted[i];
        if lod.tiles.contains(&key) || lod.pending.contains(&key) {
            continue;
        }
        // Streaming on fewer workers than there are, once playing (workers.rs).
        let Some(slot) = voxmar_engine::jobs::start(voxmar_engine::jobs::Kind::DistantLand) else { break };
        lod.pending.insert(key);
        let (generator, palette, tx) = (generator.clone(), lod.palette.clone(), lod.results_tx.clone());
        rayon::spawn(move || {
            let _slot = slot;
            let _ = tx.send(Some(build_tile(&generator, &palette, key)));
        });
    }
}

fn apply_tiles(mut lod: ResMut<Lod>) {
    for _ in 0..TILES_PER_FRAME {
        let Ok(Some(tile)) = lod.results_rx.try_recv() else { break };
        lod.pending.remove(&tile.key);
        if !lod.wanted.contains(&tile.key) {
            continue;
        }
        lod.tiles.insert(tile.key);
        lod.terrain.upload_tile(tile);
    }
}

/// What the terrain looks like at a grid corner of a tile.
#[derive(Clone, Copy)]
struct Corner {
    /// Top of the terrain: dry ground, the sea floor, or tree crowns over forests.
    height: f32,
    /// Linear albedo of the ground's top and side faces, and of tree crowns.
    top: [f32; 3],
    side: [f32; 3],
    leaves: [f32; 3],
    /// Sky light at the terrain surface, 0..=15. It fades with depth under water.
    sky: u8,
    /// Fraction of the ground under tree crowns.
    canopy: f32,
    /// Fraction of the ground with a small plant on it.
    plants: f32,
    /// Water or ice above the terrain: the sea, a lake or a river.
    sheet: Option<BlockId>,
    /// Height of the sheet's surface where there is one: the top of its water.
    level: f32,
    /// Grass, foliage and water colours, which tint the sheet over the terrain.
    colors: BiomeColors,
}

/// Seen from afar, leaves are darker than their texture: the gaps show the shaded inside of
/// the crown.
const CROWN_SHADE: f32 = 0.8;
/// How far crowns of a dense forest raise the canopy above the ground (as in `lod.wgsl`).
const CANOPY_HEIGHT: f32 = 6.0;
/// Fraction of open grassland with grass, ferns or flowers growing on it, about what the
/// generator plants.
const GRASS_PLANTS: f32 = 0.4;
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A slope past any the surface rules tell apart, which always takes a column's cliff rock.
const CLIFF_SLOPE: i32 = 64;
/// The corner and the blocks around it, half a cell out, whose cliff rock a steep corner's
/// risers blend.
const AROUND: [(i32, i32); 9] = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, 1), (1, -1), (-1, -1)];
/// Depth of the walls hanging from tile edges, hiding cracks between rings.
const SKIRT: f32 = 24.0;
/// Water and ice sheets lie a hair below the top of real water, so dry ground at the
/// water's level stays above them.
const SHEET_SINK: f32 = 1.0 / 32.0;
/// Vertex kinds, see `lod.wgsl`.
const TERRAIN: u32 = 0;
const WATER: u32 = 1;
const ICE: u32 = 2;

/// Samples the generator at the tile's grid corners and builds a smooth heightfield mesh of
/// the ground, plus a flat sheet over cells with water or ice.
fn build_tile(generator: &Overworld, palette: &LodPalette, key: TileKey) -> TileMesh {
    let _span = info_span!("lod_tile").entered();
    let cell = key.cell();
    let origin = key.origin();
    // Corners 0..=CELLS plus one more on each side for normals.
    const W: i32 = CELLS + 3;
    let at = |i: i32| (i - 1) * cell;
    let heights: Vec<i32> =
        (0..W * W).map(|i| generator.height(origin.x + at(i % W), origin.y + at(i / W))).collect();
    let height = |x: i32, z: i32| heights[((z + 1) * W + x + 1) as usize];
    // The regional climate is shared by every sample of the tile, and of the cliffs' rock
    // half a cell beyond its outermost corners.
    let sampler = generator.sampler(origin - 2 * cell, origin + (CELLS + 2) * cell);

    let mut corners = Vec::with_capacity((W * W) as usize);
    for z in -1..=CELLS + 1 {
        for x in -1..=CELLS + 1 {
            let (wx, wz) = (origin.x + x * cell, origin.y + z * cell);
            let h = height(x, z);
            // The steepest rise to a neighbouring corner, and the steepness per block,
            // rounded like the whole steps between real columns.
            let rise = if (0..=CELLS).contains(&x) && (0..=CELLS).contains(&z) {
                [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().map(|(dx, dz)| (height(x + dx, z + dz) - h).abs()).max().unwrap_or(0)
            } else {
                0
            };
            let slope = (rise + cell / 2) / cell;
            let surface = sampler.surface(wx, wz, h, slope);
            let (top, mut side) = palette.faces(surface.ground, &surface.colors);
            // Cliffs are rock: real chunks lay their cliff rock wherever a column is steep
            // from one block to the next, but a corner's slope is its cell's average, which
            // calls even a sheer cliff a gentle rise once cells are a few blocks wide. Risers
            // there showed the ground of the cliff's top or foot instead (grass, a beach's
            // sand), in stripes of whatever each corner sampled. They take the rock the
            // surface lays on a cliff instead, in as far as the cell is steep: the rock of
            // the highest corner around, whose column the cliff face below it belongs to,
            // blended over the blocks around it, as its mix of rocks blends from afar.
            let steepness = smoothstep(1.0, 2.5, rise as f32 / cell as f32);
            if steepness > 0.0 {
                let (top_x, top_z) = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
                    .into_iter()
                    .map(|(dx, dz)| (x + dx, z + dz))
                    .max_by_key(|&(x, z)| height(x, z))
                    .unwrap_or((x, z));
                let (cx, cz, ch) = (origin.x + top_x * cell, origin.y + top_z * cell, height(top_x, top_z));
                let r = cell / 2;
                let mut rock = [0.0f32; 3];
                for (dx, dz) in AROUND {
                    let face = sampler.surface(cx + dx * r, cz + dz * r, ch, CLIFF_SLOPE);
                    let colour = palette.faces(face.ground, &face.colors).1;
                    rock = std::array::from_fn(|c| rock[c] + colour[c] / AROUND.len() as f32);
                }
                side = std::array::from_fn(|c| side[c] + (rock[c] - side[c]) * steepness);
            }
            let (leaves, cover) = surface.canopy.unwrap_or((surface.ground, 0.0));
            corners.push(Corner {
                height: h as f32 + 1.0 + CANOPY_HEIGHT * cover,
                top,
                side,
                leaves: palette.color(leaves, true, &surface.colors).map(|c| c * CROWN_SHADE),
                // Sky light fades by two levels per block of water or ice.
                sky: if surface.water.is_some() { (15 - 2 * (surface.water_level - h).max(0)).max(0) as u8 } else { 15 },
                canopy: cover,
                plants: if palette.is_grassy(surface.ground) { GRASS_PLANTS * (1.0 - cover) } else { 0.0 },
                sheet: surface.water,
                level: (surface.water_level + 1) as f32 - SHEET_SINK,
                colors: surface.colors,
            });
        }
    }
    let corner = |x: i32, z: i32| corners[((z + 1) * W + x + 1) as usize];

    let pack_color = |c: [f32; 3]| linear_to_srgb_byte(c[0]) | linear_to_srgb_byte(c[1]) << 8 | linear_to_srgb_byte(c[2]) << 16;
    let pack_rgb565 = |c: [f32; 3]| linear_to_srgb_byte(c[0]) >> 3 | (linear_to_srgb_byte(c[1]) >> 2) << 5 | (linear_to_srgb_byte(c[2]) >> 3) << 11;
    let pack_position = |x: i32, z: i32, height: f32, sky: u8, kind: u32| {
        let y = (height * 32.0).round().clamp(0.0, 16383.0) as u32;
        x as u32 | (z as u32) << 6 | y << 12 | (sky as u32) << 26 | kind << 30
    };
    let terrain_vertex = |x: i32, z: i32, height: f32, normal: Vec3, c: Corner| {
        [
            pack_position(x, z, height, c.sky, TERRAIN),
            pack_color(c.top) | ((c.canopy * 255.0) as u32) << 24,
            encode_normal(normal) | pack_rgb565(c.leaves) << 16,
            pack_color(c.side) | ((c.plants * 255.0) as u32) << 24,
        ]
    };
    let mut vertices: Vec<[u32; 4]> = Vec::with_capacity(((CELLS + 1) * (CELLS + 1) + 4 * (CELLS + 1)) as usize);
    for z in 0..=CELLS {
        for x in 0..=CELLS {
            // The ground's normal, which shades its steps; crowns are shaded on their own.
            let c = corner(x, z);
            let dx = height(x + 1, z) - height(x - 1, z);
            let dz = height(x, z + 1) - height(x, z - 1);
            let normal = Vec3::new(-dx as f32, 2.0 * cell as f32, -dz as f32).normalize();
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

    // The sheet covers every cell with water or ice at a corner, at the level of its water
    // (the sea, or a lake's or river's own); dry ground pokes through it along the shore.
    // Its vertices carry the floor beneath as their side colour.
    let mut sheet = vec![u32::MAX; ((CELLS + 1) * (CELLS + 1)) as usize];
    let mut sheet_top = f32::MIN;
    for z in 0..CELLS {
        for x in 0..CELLS {
            let cell_corners = [(x, z), (x + 1, z), (x + 1, z + 1), (x, z + 1)];
            let Some(wet) = cell_corners.iter().map(|&(x, z)| corner(x, z)).find(|c| c.sheet.is_some()) else { continue };
            for (x, z) in cell_corners {
                let slot = &mut sheet[index(x, z) as usize];
                if *slot == u32::MAX {
                    *slot = vertices.len() as u32;
                    let c = corner(x, z);
                    // Dry corners take the level and kind of the water beside them.
                    let (block, level) = match c.sheet {
                        Some(block) => (block, c.level),
                        None => (wet.sheet.expect("found wet"), wet.level),
                    };
                    sheet_top = sheet_top.max(level);
                    // Water carries the biome's water colour itself, which real water is shaded
                    // from; ice its own colour and opacity.
                    let (kind, color, opacity) = if palette.is_water(block) {
                        (WATER, c.colors.water.map(srgb_to_linear), 1.0)
                    } else {
                        (ICE, palette.color(block, true, &c.colors), palette.opacity[block.0 as usize])
                    };
                    vertices.push([
                        pack_position(x, z, level, c.sky, kind),
                        pack_color(color) | ((opacity * 255.0) as u32) << 24,
                        encode_normal(Vec3::Y),
                        pack_color(c.top),
                    ]);
                }
            }
            let [a, b, c, d] = cell_corners.map(|(x, z)| sheet[index(x, z) as usize]);
            indices.extend([a, d, c, a, c, b]);
        }
    }

    let low = corners.iter().map(|c| c.height).fold(f32::MAX, f32::min) - SKIRT;
    let high = corners.iter().map(|c| c.height).fold(sheet_top, f32::max);
    let heights = low..high;
    TileMesh { key, heights: heights.start.floor() as i32..heights.end.ceil() as i32 + 1, vertices, indices }
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
