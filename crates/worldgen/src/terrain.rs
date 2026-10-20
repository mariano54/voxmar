//! Stage 4: turns sites and resources into blocks: surface materials, caves (see
//! [`crate::caves`]), water and ice at the level of the sea, lake or river over each
//! column, then trees and ground decorations (see [`crate::features`]), the small plants on
//! the ground, and last the snow blanket: where snow lies, it lies on every level open to
//! the sky down through the canopy (Wilder Wild's), on rocks and in the crowns of trees.
//!
//! Surface materials come from the biome's surface rules (see `surface`), around a few
//! physical rules every biome shares: bare rock on cliffs and exposed heights, snow where
//! it is cold enough, ice on cold water. Every threshold is a band rather than a line:
//! within it, the two materials interleave with a probability that ramps across the band,
//! in patches and speckles, so the ground changes gradually, the way the climate does.
//! Below the surface lie the stones of the biome or of its rock province.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::chunk::Chunk;
use voxmar_engine::coords::{CHUNK_SIZE, CHUNK_VOLUME, COLUMN_CHUNKS, ColumnPos, WORLD_HEIGHT, local_index};
use voxmar_engine::fluid::{self, Fluid, FluidState};
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

use crate::biome::Biomes;
use crate::caves::Cave;
use crate::content::WorldContent;
use crate::ecology::{Ground, Plant};
use crate::features::{self, Features};
use crate::geology::{GeoGrid, noise};
use crate::hoards::{Floor, Hoards};
use crate::lattice::{self, Lattice, Profile};
use crate::ores::Ores;
use crate::rng::hash3;
use crate::rocks::Rocks;
use crate::stage::{self, Stage};
use crate::surface::{Fill, Layers, Palette, Spot, Surface};
use crate::trails::Marker;
use crate::trees::TreeId;
use crate::underground::Rock;
use crate::{Body, Overworld, SEA_LEVEL, Sampler, Site};

/// Stone turns to deepslate below this height, 48 blocks under sea level.
const DEEPSLATE: i32 = SEA_LEVEL - 48;

struct Ids {
    bedrock: BlockId,
    deepslate: BlockId,
    grass: BlockId,
    water: BlockId,
    lava: BlockId,
    /// The crust of a dry salt flat.
    salt: BlockId,
    /// Boulders by creeks and rivers.
    /// The top of a damp creek boulder.
    mossy_boulder: BlockId,
    sand: BlockId,
    snow: BlockId,
    powder: BlockId,
    snowy_ground: BlockId,
    ice: BlockId,
    thick_ice: BlockId,
    rare_ice: BlockId,
}

pub struct Terrain {
    ids: Ids,
    /// Water's states: rivers' pinned water with its current, partial tops, and the
    /// flowing and falling water of a fall's curtain.
    water: Fluid,
    rock: FastNoiseLite,
    /// Patches that dither the borders between surface materials.
    patches: FastNoiseLite,
    /// Larger patches for choosing among mixed blocks.
    mixes: FastNoiseLite,
    /// Whether wood may replace a block: air, water, leaves and small plants, by block id.
    soft: Vec<bool>,
    /// Whether a block is the ground or rock a tree's wood may grow into, where a limb
    /// meets a hillside: solid and neither wood nor leaves, by block id.
    earth: Vec<bool>,
    /// Whether ore may replace a block: every stone, by block id.
    stone: Vec<bool>,
    /// What each block offers plants, by block id.
    grounds: Vec<Ground>,
    /// Whether a block is a small plant, by block id.
    plants: Vec<bool>,
    /// Whether a block holds up what stands on it: solid, and no plant, by block id.
    firm: Vec<bool>,
    /// What the snow blanket makes of a block open to the sky, by block id.
    blanket: Vec<Option<BlockId>>,
    /// What lying snow makes of a ground block, by block id (`snow_cover` in weather.ron).
    snow_cover: Vec<Option<BlockId>>,
    seed: u64,
    /// Ore veins, and the samples of their ore over them.
    pub(crate) ores: Ores,
    /// Locked treasure chests, left on floors under rock.
    pub(crate) hoards: Hoards,
    pub(crate) features: Features,
    /// Trail treads, by the biome tags they suit (none: everywhere); bridge decks and
    /// stepping stones.
    treads: Vec<(Vec<String>, Palette)>,
    worn_edge: BlockId,
    deck: BlockId,
    log_deck: BlockId,
    stepping_stone: BlockId,
    cairn: Palette,
    waymarker: Palette,
}

impl Terrain {
    pub fn new(seed: u64, registry: &BlockRegistry, content: &WorldContent, rocks: &Rocks, biomes: &Biomes) -> Self {
        let id = |name: &str| registry.expect(&format!("base:{name}"));
        let frost = &content.climate.frost;
        let water = id("water");
        let by_id = |f: &dyn Fn(BlockId) -> bool| (0..registry.len()).map(|i| f(BlockId(i as u16))).collect::<Vec<bool>>();
        let water_fluid = registry.fluid_def(registry.fluid(water).expect("water is a fluid").fluid).clone();
        // Wood grows through standing water and rivers, never through falling or flowing
        // water (see also `features`: no tree in water by a fall).
        let soft = by_id(&|block| {
            let def = registry.def(block);
            block.is_air()
                || registry.fluid(block).is_some_and(|s| s.fluid == water_fluid.id && s.source)
                || def.name.ends_with("_leaves")
                || def.render == voxmar_engine::block::RenderKind::Plant
        });
        let mut stones: Vec<BlockId> = rocks.stones().chain([id("stone"), id("deepslate")]).collect();
        stones.extend(biomes.iter().filter_map(|(_, b)| b.stones.as_ref()).flat_map(|p| p.blocks()));
        // What a block offers plants follows its tags: snow over soil, then soil, sand.
        let grounds = (0..registry.len())
            .map(|i| {
                let def = registry.def(BlockId(i as u16));
                match () {
                    _ if def.has_tag("snow") => Ground::Snow,
                    _ if def.has_tag("soil") => Ground::Soil,
                    _ if def.has_tag("sand") => Ground::Sand,
                    _ => Ground::Bare,
                }
            })
            .collect();
        let plants = by_id(&|block| registry.def(block).render == voxmar_engine::block::RenderKind::Plant);
        let earth = by_id(&|block| earth(registry, block));
        let firm = by_id(&|block| registry.def(block).solid && registry.def(block).render != voxmar_engine::block::RenderKind::Plant);
        let blanket = (0..registry.len())
            .map(|i| {
                let def = registry.def(BlockId(i as u16));
                frost.blanket.iter().find(|rule| rule.applies(def)).map(|rule| registry.expect(&rule.to))
            })
            .collect();
        let snow_cover = (0..registry.len())
            .map(|i| content.snow_cover.iter().find(|(ground, _)| registry.def(BlockId(i as u16)).name == *ground).map(|(_, snowy)| registry.expect(snowy)))
            .collect();
        Self {
            water: water_fluid,
            ids: Ids {
                bedrock: id("bedrock"),
                deepslate: id("deepslate"),
                grass: id("grass_block"),
                water,
                lava: id("lava"),
                salt: id("calcite"),
                mossy_boulder: id("moss_block"),
                sand: id("sand"),
                snow: registry.expect(&frost.snow),
                powder: registry.expect(&frost.powder),
                snowy_ground: registry.expect(&frost.snowy_ground),
                ice: registry.expect(&frost.ice),
                thick_ice: registry.expect(&frost.thick_ice),
                rare_ice: registry.expect(&frost.rare_ice),
            },
            rock: noise(seed, 23, 0.02, FractalType::None, 1),
            patches: noise(seed, 24, 0.04, FractalType::FBm, 2),
            mixes: noise(seed, 25, 1.0 / 9.0, FractalType::FBm, 2),
            soft,
            earth,
            // Ores replace plain stone, never gems: a crystal cave's walls, a geode's lining.
            stone: by_id(&|block| stones.contains(&block) && !registry.def(block).has_tag("gem")),
            grounds,
            plants,
            firm,
            blanket,
            snow_cover,
            seed,
            ores: Ores::new(content, registry, rocks),
            hoards: Hoards::new(&content.hoards, registry),
            features: Features::new(content, registry),
            treads: content.trails.tread.iter().map(|t| (t.biome.clone(), Palette::new(&t.blocks, registry))).collect(),
            worn_edge: registry.expect(&content.trails.edge),
            deck: registry.expect(&content.trails.bridge),
            log_deck: registry.expect(&content.trails.log),
            stepping_stone: registry.expect(&content.trails.stepping_stone),
            cairn: Palette::new(&content.trails.cairn, registry),
            waymarker: Palette::new(&content.trails.waymarker, registry),
        }
    }
}

/// Dense, mutable blocks of the column being generated, addressed in world coordinates.
pub struct Blocks<'a> {
    origin: IVec3,
    data: Vec<BlockId>,
    stone: &'a [bool],
    soft: &'a [bool],
    earth: &'a [bool],
}

#[cfg(test)]
impl<'a> Blocks<'a> {
    /// An empty column at `origin`, for tests: wood grows through what `soft` marks, ore
    /// replaces what `stone` marks (both by block id).
    pub(crate) fn empty(origin: IVec3, stone: &'a [bool], soft: &'a [bool], earth: &'a [bool]) -> Self {
        Self { origin, data: vec![BlockId::AIR; CHUNK_VOLUME * COLUMN_CHUNKS as usize], stone, soft, earth }
    }
}

impl Blocks<'_> {
    fn index(&self, world: IVec3) -> Option<usize> {
        let p = world - self.origin;
        let inside = (0..CHUNK_SIZE).contains(&p.x) && (0..CHUNK_SIZE).contains(&p.z) && (0..WORLD_HEIGHT).contains(&p.y);
        inside.then(|| Self::local(p.x, p.y, p.z))
    }

    fn local(x: i32, y: i32, z: i32) -> usize {
        (y / CHUNK_SIZE) as usize * CHUNK_VOLUME + local_index(IVec3::new(x, y % CHUNK_SIZE, z))
    }

    pub(crate) fn get_local(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.data[Self::local(x, y, z)]
    }

    pub(crate) fn set_local(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
        self.data[Self::local(x, y, z)] = block;
    }

    /// The part of the box from `min` to `max` (inclusive) inside the column, if any.
    pub fn clip(&self, min: IVec3, max: IVec3) -> Option<(IVec3, IVec3)> {
        let low = min.max(self.origin);
        let high = max.min(self.origin + IVec3::new(CHUNK_SIZE - 1, WORLD_HEIGHT - 1, CHUNK_SIZE - 1));
        low.cmple(high).all().then_some((low, high))
    }

    /// Sets the block at `pos`, if it is in the column.
    pub fn set(&mut self, pos: IVec3, block: BlockId) {
        if let Some(i) = self.index(pos) {
            self.data[i] = block;
        }
    }

    /// Whether `pos` lies in the column.
    pub fn contains(&self, pos: IVec3) -> bool {
        self.index(pos).is_some()
    }

    /// Whether `pos` holds ground or rock a tree could stand on (and false outside the
    /// column).
    pub fn earth(&self, pos: IVec3) -> bool {
        self.index(pos).is_some_and(|i| self.earth[self.data[i].0 as usize])
    }

    /// Whether (x, z) lies within `margin` blocks of the column, sideways.
    pub fn near(&self, x: i32, z: i32, margin: i32) -> bool {
        let (x, z) = (x - self.origin.x, z - self.origin.z);
        (-margin..CHUNK_SIZE + margin).contains(&x) && (-margin..CHUNK_SIZE + margin).contains(&z)
    }

    /// The block at `pos`; air outside the column.
    pub fn get(&self, pos: IVec3) -> BlockId {
        self.index(pos).map_or(BlockId::AIR, |i| self.data[i])
    }

    /// Puts `block` at `pos` if it is air there; whether it did.
    pub fn put_if_air(&mut self, pos: IVec3, block: BlockId) -> bool {
        if let Some(i) = self.index(pos)
            && self.data[i].is_air()
        {
            self.data[i] = block;
            return true;
        }
        false
    }

    /// Whether wood may grow into `pos`: air, water, leaves or a small plant (and false
    /// outside the column).
    pub fn soft(&self, pos: IVec3) -> bool {
        self.index(pos).is_some_and(|i| self.soft[self.data[i].0 as usize])
    }

    /// A tree's wood grows through air, water, leaves and small plants, and into the
    /// ground or rock where a limb meets a hillside (so its far end never hangs in the
    /// air); whether it did.
    pub fn put_wood(&mut self, pos: IVec3, block: BlockId) -> bool {
        if let Some(i) = self.index(pos)
            && (self.soft[self.data[i].0 as usize] || self.earth[self.data[i].0 as usize])
        {
            self.data[i] = block;
            return true;
        }
        false
    }

    /// Wood grows through air, water, leaves and small plants; whether it did.
    pub fn put_log(&mut self, pos: IVec3, block: BlockId) -> bool {
        if let Some(i) = self.index(pos)
            && self.soft[self.data[i].0 as usize]
        {
            self.data[i] = block;
            return true;
        }
        false
    }

    pub fn replace_stone(&mut self, pos: IVec3, block: BlockId) {
        if let Some(i) = self.index(pos)
            && self.stone[self.data[i].0 as usize]
        {
            self.data[i] = block;
        }
    }
}

pub fn generate(world: &Overworld, pos: ColumnPos) -> Column {
    let terrain = &world.terrain;
    let ids = &terrain.ids;
    let origin = pos.origin();
    let corner = IVec2::new(origin.x, origin.z);
    // Trees reach in from candidates around the column, so sample the climate there too.
    let area = world.dense_sampler(corner - features::REACH, corner + CHUNK_SIZE - 1 + features::REACH);

    // Geology for the column plus a two-block border (for slopes and steepness).
    let geology = GeoGrid::new(world, (origin.x - 2, origin.z - 2), CHUNK_SIZE + 4);
    let top = geology.samples().iter().map(|g| g.height.max(g.water)).max().unwrap_or(0).max(SEA_LEVEL);

    let mut caves = {
        let _stage = stage::enter(Stage::Caves);
        world.caves.column(&geology, origin, top)
    };
    let underground = {
        let _stage = stage::enter(Stage::CaveBiomes);
        world.underground.column(world, &area, &geology, origin, top)
    };
    // The rock noise picks among the stones above the deepslate, and anywhere among a cave
    // biome's own.
    let rock = {
        let _stage = stage::enter(Stage::Fill);
        let low = if underground.any_rocky() { 0 } else { DEEPSLATE };
        // Read in the ground only, under the highest of it.
        let ground = geology.samples().iter().map(|g| g.height).max().unwrap_or(0);
        Lattice::noise(&terrain.rock, origin, low, ground.max(low), lattice::STEP, 1.0)
    };
    let mut rock_profile = Profile::default();
    // The density's noise over the band of the column's crags, below their tops, which
    // geology has already found (`crate::density`).
    let density = world.geology.density();
    let crag_band = geology
        .samples()
        .iter()
        .filter(|g| g.crag.amplitude > 0.0)
        .map(|g| (g.crag.band(density.above).0, g.height))
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
        .filter(|band| band.0 <= band.1);
    let crags = crag_band.map(|(low, high)| Lattice::noise(&density.noise, origin, low, high, crate::density::STEP_Y, density.squash));
    let mut crag_profile = Profile::default();
    let mut hollow: Vec<bool> = Vec::new();
    let mut settled = Vec::new();
    let mut cave = Vec::with_capacity(WORLD_HEIGHT as usize);

    let mut blocks = Blocks {
        origin,
        data: vec![BlockId::AIR; CHUNK_VOLUME * COLUMN_CHUNKS as usize],
        stone: &terrain.stone,
        soft: &terrain.soft,
        earth: &terrain.earth,
    };
    let mut column = Column::default();
    let mut surfaces = Vec::with_capacity((CHUNK_SIZE * CHUNK_SIZE) as usize);

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let _fill = stage::enter(Stage::Fill);
            let (wx, wz) = (origin.x + x, origin.z + z);
            let geo = geology.get(wx, wz);
            let height = geo.height;
            let site = area.site_on(&geo, wx, wz, geology.slope(wx, wz), geology.steepness(wx, wz));
            let colors = area.colors_of(&site, wx, wz);
            column.colors[(z * CHUNK_SIZE + x) as usize] = [colors.grass, colors.foliage, colors.water];
            let surface = stage::enter(Stage::Surface);
            let layers = world.surface_layers(&site, wx, wz);
            let stones = world.stones(&site);
            let strata = world.surface_of(&site, wx, wz).1;
            let bands = world.rock_bands(&site);
            drop(surface);
            {
                let _stage = stage::enter(Stage::Caves);
                caves.carve(x, z, &mut cave);
            }
            rock.profile_to(x, z, height, &mut rock_profile);
            let rocky = underground.rocky(x, z);
            if rocky {
                underground.settled(x, z, &mut settled);
            }
            // Strata wobble the same all the way down a block column.
            let mut wobble = None;
            // The ground's own stone at a height below the surface's layers: a cave biome's
            // rock, deepslate, or the stones beneath.
            let mut stone = |y: i32| {
                let rock = match rocky.then(|| settled[(y >> 3) as usize]) {
                    Some(Some(pick)) => underground.rock_of(pick),
                    Some(None) => underground.rock(x, y, z),
                    None => None,
                };
                match rock {
                    Some(Rock::Stones(stones)) => stones.pick((0.5 + rock_profile.at(y) * 0.53).clamp(0.0, 0.9999)),
                    Some(Rock::Strata(strata)) => stratum(strata, y + *wobble.get_or_insert_with(|| terrain.wobble(wx, wz))),
                    None if y < DEEPSLATE => ids.deepslate,
                    None => stone_at(ids, stones, rock_profile.at(y), world.seed, y, wx, wz),
                }
            };
            // Below this the ground is that stone (from bedrock up): the bulk of the column,
            // taken first.
            let body = height - if layers.filler != Fill::Stones { layers.depth } else { 0 };
            // Where the density hollows the column under its top: never down to the water
            // beside it, in the open or in a river's tunnel.
            let hollow_from = match (&crags, geo.crag.amplitude > 0.0) {
                (Some(crags), true) => {
                    crags.profile(x, z, &mut crag_profile);
                    let beside = sides_of(&geology, wx, wz).iter().filter_map(|n| n.wet().map(|(_, top)| top)).max().unwrap_or(i32::MIN);
                    let low = geo.crag.band(density.above).0.max(beside.saturating_add(2));
                    density.hollows(&geo.crag, &crag_profile, low, height, &mut hollow);
                    low
                }
                _ => {
                    hollow.clear();
                    height + 1
                }
            };
            // The window under an arch: open from its foot, leaving at least four blocks of
            // the fin (as the land eased it) over it, where no water beside it could pour in.
            let foot = geo.form.base.floor() as i32;
            // A tower, spire or arch is one rock from its foot up: the rock province's beds
            // (its main stone in shades), not whatever the surface rules of each column
            // under it would pick, so it never reads as pieces of different stone. Its top
            // keeps the ground's own cap.
            let province_beds = &world.rocks.get(site.province).bands;
            let feature_rock = height >= foot + 2 && !province_beds.is_empty() && geo.form.shape.is_some_and(|s| world.geology.shapes().feature(s));
            let window = (geo.form.window as i32).min(height - foot - 4);
            let beside_water = || sides_of(&geology, wx, wz).iter().any(|n| n.wet().is_some_and(|(_, top)| top > foot));
            let window = if window >= 3 && !geo.submerged() && !beside_water() { window } else { 0 };
            let hollow_at = |y: i32| (y > foot && y <= foot + window) || (y >= hollow_from && hollow.get((y - hollow_from) as usize).copied().unwrap_or(false));

            for y in 0..=height {
                let below_surface = height - y;
                if hollow_at(y) {
                    continue;
                }
                match cave[y as usize] {
                    Cave::Rock => {}
                    Cave::Air => continue,
                    Cave::Water => {
                        blocks.set_local(x, y, z, ids.water);
                        continue;
                    }
                    Cave::Lava => {
                        blocks.set_local(x, y, z, ids.lava);
                        continue;
                    }
                }
                let block = if feature_rock && y > foot && y < height && !hollow_at(y + 1) {
                    terrain.band(province_beds, y, wx, wz)
                } else if y >= 4 && y < body {
                    // A ledge under an overhang takes the surface's top, as the top does.
                    if hollow_at(y + 1) && !hollow_at(y - 1) { layers.top } else { stone(y) }
                } else if y == 0 || (y < 4 && hash3(world.seed, wx, y, wz) % 4 >= y as u64) {
                    ids.bedrock
                } else if hollow_at(y - 1) {
                    // The underside of an overhang is rock: nothing loose hangs.
                    stone(y)
                } else if below_surface == 0 || hollow_at(y + 1) {
                    layers.top
                } else if below_surface <= layers.depth && layers.filler != Fill::Stones {
                    match layers.filler {
                        Fill::Block(block) => block,
                        Fill::Rock | Fill::Beds => terrain.band(bands, y, wx, wz),
                        _ => terrain.band(strata, y, wx, wz),
                    }
                } else {
                    stone(y)
                };
                blocks.set_local(x, y, z, block);
            }
            {
                let _stage = stage::enter(Stage::CaveBiomes);
                underground.decorate(x, z, &site, &cave, &mut blocks);
                underground.bury(x, z, &cave, &mut blocks);
            }
            // Rivers' water is pinned with its current, and every body's top block holds its
            // surface's eighths. Fast water stays open in the cold, and so does water a fall
            // pours into.
            let sides = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| geology.get(wx + dx, wz + dz));
            let fall = sides.iter().any(|n| n.wet().is_some_and(|(_, top)| top >= geo.water + 2));
            let ice = terrain
                .ice(&site, wx, wz)
                .filter(|_| geo.current.is_none_or(|c| c.speed < 3) && geo.body != Body::Spring && !fall);
            for y in height + 1..=geo.water {
                let block = match y == geo.water {
                    true => ice.unwrap_or_else(|| terrain.water_block(geo.top, geo.current)),
                    false => terrain.water_block(8, geo.current),
                };
                blocks.set_local(x, y, z, block);
            }
            // Rocks break the surface of rapids, though not by a step in the water, where
            // the water on one side must meet water on the other.
            if geo.current.is_some_and(|c| c.speed >= 3)
                && matches!(geo.body, Body::Creek | Body::River)
                && hash3(world.seed ^ 0xb01d, wx, 0, wz) % 100 < 9
                && sides.iter().all(|n| n.wet().is_none_or(|(_, top)| top == geo.water))
            {
                let rock = stones.pick((hash3(world.seed ^ 0xb01e, wx, 0, wz) & 0xffff) as f32 / 65536.0);
                for y in height + 1..=geo.water {
                    blocks.set_local(x, y, z, rock);
                }
            }
            // Trails cross water on decks and stepping stones.
            if let Some(trail) = geo.trail {
                if trail.stone && height == geo.water && !geo.submerged() {
                    blocks.set_local(x, height, z, terrain.stepping_stone);
                }
                // No deck where a fall pours in: falling water must land on water, and would
                // spread over the planks.
                if let Some(deck) = trail.deck.filter(|&d| d > height && d < WORLD_HEIGHT && !fall) {
                    blocks.set_local(x, deck, z, if trail.log { terrain.log_deck } else { terrain.deck });
                    if trail.pier {
                        for y in height + 1..deck {
                            blocks.set_local(x, y, z, terrain.log_deck);
                        }
                    }
                }
            }
            // A river running in a tunnel under the ground.
            if let Some(cavity) = geo.cavity {
                for y in cavity.floor + 1..=cavity.ceiling.min(height - 3) {
                    let block = match y.cmp(&geo.water) {
                        std::cmp::Ordering::Less => terrain.water_block(8, geo.current),
                        std::cmp::Ordering::Equal => terrain.water_block(geo.top, geo.current),
                        std::cmp::Ordering::Greater => BlockId::AIR,
                    };
                    blocks.set_local(x, y, z, block);
                }
            }
            surfaces.push((site, layers.top));
        }
    }

    {
        let _stage = stage::enter(Stage::Shapes);
        notches(world, &geology, origin, &surfaces, &mut blocks);
    }
    {
        let _stage = stage::enter(Stage::Ores);
        terrain.ores.place(world, pos, &mut blocks);
    }
    let decoration = stage::enter(Stage::Decoration);
    terrain.features.place(world, &area, &geology, &caves, pos, &mut blocks, &mut column.colors);
    {
        let _stage = stage::enter(Stage::Trails);
        terrain.markers(world, &area, &geology, pos, &mut blocks);
    }
    terrain.curtains(&geology, origin, &mut blocks);

    // Ground plants, wherever trees, decorations, caves and water left the ground open:
    // first boulders and the ground each open column offers, then the patches of rare herbs
    // along water, which the chunk's columns decide together (a chunk is one cell of them),
    // then a plant for every other open column.
    const _: () = assert!(crate::ecology::PATCH_CELL == CHUNK_SIZE);
    let mut open_ground: Vec<Option<Ground>> = vec![None; (CHUNK_SIZE * CHUNK_SIZE) as usize];
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (site, _) = &surfaces[(z * CHUNK_SIZE + x) as usize];
            let h = site.height;
            if h + 1 >= WORLD_HEIGHT {
                continue;
            }
            let Some(ground) = terrain.plant_ground(site, blocks.get_local(x, h, z), blocks.get_local(x, h + 1, z)) else { continue };
            if let Some(boulder) = boulder(world, site, &geology, origin.x + x, origin.z + z) {
                for (y, block) in (h + 1..).zip(boulder.into_iter().flatten()) {
                    if y >= WORLD_HEIGHT || !blocks.get_local(x, y, z).is_air() {
                        break;
                    }
                    blocks.set_local(x, y, z, block);
                }
                continue;
            }
            open_ground[(z * CHUNK_SIZE + x) as usize] = Some(ground);
        }
    }
    let edges = world.ecology.edge_patches(IVec2::new(origin.x, origin.z), |i| open_ground[i].map(|ground| (&surfaces[i].0, ground)));
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let i = (z * CHUNK_SIZE + x) as usize;
            let Some(ground) = open_ground[i] else { continue };
            let site = &surfaces[i].0;
            let h = site.height;
            let open = |y: i32| y < WORLD_HEIGHT && blocks.get_local(x, y, z).is_air();
            let plant = match edges.iter().find(|(j, _)| *j == i) {
                Some(&(_, plant)) => Some(plant),
                None => world.ecology.plant(site, ground, origin.x + x, origin.z + z),
            };
            match plant {
                Some(Plant::One(plant)) => blocks.set_local(x, h + 1, z, plant),
                Some(Plant::Tall(bottom, top)) if open(h + 2) => {
                    blocks.set_local(x, h + 1, z, bottom);
                    blocks.set_local(x, h + 2, z, top);
                }
                Some(Plant::Stack(plant, n)) => {
                    let height = (h + 1..=h + n).take_while(|&y| open(y)).count() as i32;
                    for y in h + 1..=h + height {
                        blocks.set_local(x, y, z, plant);
                    }
                }
                _ => {}
            }
        }
    }

    // Settlements' buildings, props and paving, on the ground their plans graded.
    let settled;
    {
        let _stage = stage::enter(Stage::Decoration);
        settled = world.settlements.build(world, pos, &mut blocks, &|x, z| geology.get(x, z).height);
        // Lairs, dug under the land the settlements left.
        world.lairs.build(world, origin, &mut blocks, &|x, z| geology.get(x, z).height, &|b| terrain.plants[b.0 as usize]);
    }

    // Samples of ore over the veins below, on open ground of the land's own: never in
    // water, on a path, on wood or leaves, or where the vein would lie above the ground.
    for sample in terrain.ores.samples(world, pos) {
        let (x, z) = (sample.x - origin.x, sample.z - origin.z);
        let site = &surfaces[(z * CHUNK_SIZE + x) as usize].0;
        let h = site.height;
        if h < site.water || h <= sample.floor || site.trail > 0.2 || h + 2 >= WORLD_HEIGHT {
            continue;
        }
        let ground = blocks.get_local(x, h, z);
        let (above, over) = (blocks.get_local(x, h + 1, z), blocks.get_local(x, h + 2, z));
        let open = above.is_air() || terrain.plants[above.0 as usize] && !terrain.plants[over.0 as usize];
        if terrain.firm[ground.0 as usize] && terrain.earth[ground.0 as usize] && open {
            blocks.set_local(x, h + 1, z, sample.block);
        }
    }

    // Locked chests, on a floor under rock near a try that came here: a cave's floor or a
    // ledge under an overhang, never in the open, in water, or where a settlement stands;
    // at most one a column.
    if settled.is_none() {
        let ground = |block: BlockId| terrain.firm[block.0 as usize] && terrain.earth[block.0 as usize];
        for t in terrain.hoards.tries_in(world, pos) {
            let found = Hoards::spots(&t, corner).into_iter().find_map(|(x, z)| {
                let surface = surfaces[(z * CHUNK_SIZE + x) as usize].0.height;
                let at = |y: i32| match blocks.get_local(x, y, z) {
                    block if block.is_air() => Floor::Air,
                    block if ground(block) => Floor::Ground,
                    _ => Floor::Other,
                };
                terrain.hoards.floor(t.hoard, surface, WORLD_HEIGHT - 2, at).map(|y| (x, y, z))
            });
            // One chest to a column at most: two tries near each other leave one.
            if let Some((x, y, z)) = found {
                blocks.set_local(x, y, z, terrain.hoards.chest(t.hoard));
                break;
            }
        }
    }

    // The snow blanket: on every level open to the sky where snow lies on the ground.
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (site, surface) = &surfaces[(z * CHUNK_SIZE + x) as usize];
            if !terrain.frosty(*surface) {
                continue;
            }
            // Through a canopy snow reaches every level; under a roof or an overhang of
            // rock none falls, however much air lies between.
            let (mut open, mut sheltered) = (true, false);
            // From over the tallest tree or crag, or the tallest roof a settlement set here.
            let top = (site.height + SNOW_REACH).max(settled.map_or(0, |t| t + 2)).min(WORLD_HEIGHT);
            for y in (site.height + 1..top).rev() {
                let block = blocks.get_local(x, y, z);
                if open && !sheltered && let Some(snowy) = terrain.blanket[block.0 as usize] {
                    blocks.set_local(x, y, z, snowy);
                }
                open = block.is_air() || terrain.plants[block.0 as usize];
                sheltered |= terrain.earth[block.0 as usize];
            }
        }
    }

    drop(decoration);
    let _stage = stage::enter(Stage::Pack);
    for (cy, chunk) in column.chunks.iter_mut().enumerate() {
        *chunk = Chunk::from_dense(&blocks.data[cy * CHUNK_VOLUME..(cy + 1) * CHUNK_VOLUME]);
    }
    column
}

/// How far over the ground the snow blanket looks for a roof or overhang.
const SNOW_REACH: i32 = 48;

/// How far into a sea cliff waves cut a notch at the waterline, in blocks.
const NOTCH: i32 = 3;

/// Notches cut by the waves at the foot of sea cliffs, deepest at the face, where no fresh
/// water is near.
fn notches(world: &Overworld, geology: &GeoGrid, origin: IVec3, surfaces: &[(Site, BlockId)], blocks: &mut Blocks) {
    let terrain = &world.terrain;
    let shapes = world.geology.shapes();
    const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let site = &surfaces[(z * CHUNK_SIZE + x) as usize].0;
            if !site.form.shape.is_some_and(|s| shapes.sea_cliff(s)) || site.height < SEA_LEVEL + NOTCH + 3 || site.river > 0.0 {
                continue;
            }
            let (wx, wz) = (origin.x + x, origin.z + z);
            if terrain.patches.get_noise_2d(wx as f32 * 2.0, wz as f32 * 2.0) < -0.25 {
                continue;
            }
            let sea = |dx: i32, dz: i32| {
                let g = geology.get(wx + dx, wz + dz);
                g.body == Body::Sea && g.submerged() && g.water == SEA_LEVEL && g.cavity.is_none()
            };
            let fresh = SIDES.iter().any(|&(dx, dz)| {
                let g = geology.get(wx + dx, wz + dz);
                g.water > SEA_LEVEL && (g.submerged() || g.river > 0.0)
            });
            let Some(d) = (1..=NOTCH).find(|&d| SIDES.iter().any(|&(dx, dz)| sea(dx * d, dz * d))) else { continue };
            if fresh {
                continue;
            }
            for y in SEA_LEVEL + 1..=SEA_LEVEL + 1 + NOTCH - d {
                if blocks.get_local(x, y, z) != terrain.ids.water {
                    blocks.set_local(x, y, z, BlockId::AIR);
                }
            }
        }
    }
}

impl Terrain {
    /// A block of water filling `amount` eighths of its cell, pinned with a river's current
    /// where it has one.
    fn water_block(&self, amount: u8, current: Option<crate::Current>) -> BlockId {
        let current = current.filter(|c| c.speed > 0).map(|c| fluid::Current { direction: c.direction, speed: c.speed });
        self.water.block(FluidState { amount, current, ..FluidState::source(self.water.id) })
    }

    /// Where water stands two or more blocks above a column's, it pours down into it as the
    /// fluid rule would pour it, so generated water is a fixed point of the rule (see
    /// "Generated water" in `voxmar_engine::fluid`): each air cell beside the upper water
    /// holds the deepest flowing water, or falling water under more water, and every cell
    /// below, down to the column's own water, falling water. The upper water's top block
    /// holds an eighth (see `water::settle`), which pours nothing, so the curtain starts a
    /// block below it.
    fn curtains(&self, geology: &GeoGrid, origin: IVec3, blocks: &mut Blocks) {
        let (flowing, falling) = (self.water.flowing(7), self.water.falling());
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let (wx, wz) = (origin.x + x, origin.z + z);
                let Some((bottom, top)) = geology.get(wx, wz).wet() else { continue };
                let upper: Vec<(i32, i32)> = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .into_iter()
                    .filter_map(|(dx, dz)| geology.get(wx + dx, wz + dz).wet())
                    .filter(|&(_, above)| above >= top + 2)
                    .collect();
                let Some(highest) = upper.iter().map(|&(_, above)| above).max() else { continue };
                let mut pouring = false;
                for y in (top + 1..highest).rev() {
                    let open = blocks.get_local(x, y, z).is_air();
                    let beside = upper.iter().any(|&(bottom, above)| bottom < y && y < above);
                    // A curtain pours through anything but ground and rock: trees grow before
                    // it, and a mangrove's roots in its way left falling water standing on wood.
                    pouring = if pouring && !self.earth[blocks.get_local(x, y, z).0 as usize] {
                        blocks.set_local(x, y, z, falling);
                        true
                    } else if beside && open {
                        blocks.set_local(x, y, z, flowing);
                        true
                    } else {
                        false
                    };
                }
                // Where it lands, the column's own water must be water: a tree grown before
                // the curtain may have sunk its roots through it.
                if pouring {
                    for y in (bottom + 1..=top).rev() {
                        let block = blocks.get_local(x, y, z);
                        if self.earth[block.0 as usize] {
                            break;
                        }
                        if block.is_air() || !self.soft[block.0 as usize] {
                            blocks.set_local(x, y, z, self.ids.water);
                        }
                    }
                }
            }
        }
    }

    /// Builds what stands beside trails in and around a column: cairns where trees do not
    /// grow, larger cairns on summits, and old waymarkers at trailheads. Each stone stands
    /// on its own column's ground, so they settle onto uneven ground.
    fn markers(&self, world: &Overworld, area: &Sampler, geology: &GeoGrid, pos: ColumnPos, blocks: &mut Blocks) {
        let origin = pos.origin();
        let corner = IVec2::new(origin.x, origin.z);
        for (at, marker) in world.trail_markers(corner - 2, corner + CHUNK_SIZE + 1) {
            let geo = geology.get(at.x, at.y);
            if geo.submerged() {
                continue;
            }
            let roll = |salt: u64| hash3(self.seed ^ salt, at.x, 0, at.y);
            let mut stone = |dx: i32, dz: i32, up: i32| {
                let ground = geology.get(at.x + dx, at.y + dz);
                if ground.submerged() {
                    return;
                }
                let palette = if marker == Marker::Waymarker { &self.waymarker } else { &self.cairn };
                let u = (hash3(self.seed ^ 0xca1, at.x + dx, up, at.y + dz) >> 40) as f32 / (1u64 << 24) as f32;
                blocks.put_if_air(IVec3::new(at.x + dx, ground.height + up, at.y + dz), palette.pick(u * 0.9999));
            };
            match marker {
                Marker::Cairn => {
                    // Where trees grow, the forest marks the way well enough.
                    let site = area.site_on(&geo, at.x, at.y, 0, 0.0);
                    let bare = site.forest < 0.05 && (site.height - SEA_LEVEL > 60 || site.temperature < 2.0 || site.vegetation < 0.2);
                    if !bare {
                        continue;
                    }
                    for up in 1..=2 + (roll(1) % 2) as i32 {
                        stone(0, 0, up);
                    }
                    let (dx, dz) = [(1, 0), (-1, 0), (0, 1), (0, -1)][(roll(2) % 4) as usize];
                    stone(dx, dz, 1);
                }
                Marker::Summit => {
                    for up in 1..=3 + (roll(3) % 2) as i32 {
                        stone(0, 0, up);
                    }
                    for (i, (dx, dz)) in [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().enumerate() {
                        for up in 1..=1 + ((roll(4 + i as u64) % 3) as i32).min(1) {
                            stone(dx, dz, up);
                        }
                    }
                }
                Marker::Waymarker => {
                    // Weathered down to one to three stones, with one fallen beside it.
                    for up in 1..=1 + (roll(5) % 3) as i32 {
                        stone(0, 0, up);
                    }
                    let (dx, dz) = [(2, 0), (-2, 1), (1, 2), (-1, -2)][(roll(6) % 4) as usize];
                    stone(dx, dz, 1);
                }
            }
        }
    }

    /// A uniform-ish 0..1 value for dithering thresholds: smooth patches with a little
    /// per-block roughness at their edges, so materials meet in clumps and wavering lines
    /// rather than interleaving block by block.
    pub(crate) fn dither(&self, x: i32, z: i32) -> f32 {
        let speckle = (hash3(self.seed ^ 0x5fac, x, 0, z) & 0xffff) as f32 / 65536.0;
        let patch = self.patches.get_noise_2d(x as f32, z as f32) * 0.6 + 0.5;
        (0.96 * patch + 0.04 * speckle).clamp(0.0, 1.0)
    }

    /// Uniform per-column randomness for the surface rules.
    pub(crate) fn chance(&self, x: i32, z: i32) -> f32 {
        (hash3(self.seed ^ 0xc4a, x, 0, z) & 0xffff) as f32 / 65536.0
    }

    /// Another clumpy value in 0..1, independent of `dither`, for choosing among mixed
    /// blocks so that each forms patches with slightly ragged edges.
    pub(crate) fn mix(&self, x: i32, z: i32) -> f32 {
        let speckle = (hash3(self.seed ^ 0x3a1f, x, 0, z) & 0xffff) as f32 / 65536.0;
        let patch = crate::climate::uniform(self.mixes.get_noise_2d(x as f32, z as f32));
        (0.97 * patch + 0.03 * speckle).clamp(0.0, 0.9999)
    }

    /// Strata: bands of rock, one block each, that wobble gently.
    fn band(&self, strata: &[BlockId], y: i32, x: i32, z: i32) -> BlockId {
        stratum(strata, y + self.wobble(x, z))
    }

    /// How far strata are shifted up or down at (x, z).
    fn wobble(&self, x: i32, z: i32) -> i32 {
        (self.patches.get_noise_2d(x as f32 * 0.25, z as f32 * 0.25) * 2.5).round() as i32
    }

    /// The frozen surface of sea, lake or river water at this site, if it freezes: ice
    /// that breaks up into floes at its edge, thick ice where it is very cold.
    fn ice(&self, site: &Site, x: i32, z: i32) -> Option<BlockId> {
        let t = site.temperature + 1.5 * self.patches.get_noise_2d(x as f32, z as f32);
        let d = self.dither(x, z);
        match t {
            t if t >= -1.0 => None,
            t if -t > 7.0 + 4.0 * d => Some(if d > 0.97 { self.ids.rare_ice } else { self.ids.thick_ice }),
            _ => Some(self.ids.ice),
        }
    }

    /// The worn ground of a trail through a biome with these tags, in patches.
    fn tread(&self, tags: &[String], x: i32, z: i32) -> BlockId {
        let (_, palette) = self.treads.iter().find(|(want, _)| want.is_empty() || want.iter().any(|t| tags.contains(t))).expect("the last tread suits every biome");
        palette.pick(self.mix(x, z))
    }

    pub fn leaves(&self, tree: TreeId) -> BlockId {
        self.features.trees().leaves(tree)
    }

    pub fn is_grass(&self, block: BlockId) -> bool {
        block == self.ids.grass
    }

    /// What a surface block offers plants.
    pub fn ground(&self, block: BlockId) -> Ground {
        self.grounds[block.0 as usize]
    }

    /// What the actual surface offers a ground plant. Cave mouths can remove the
    /// nominal surface: air, water and wood also classify as Bare, but are not earth.
    /// Both ordinary plants and bank patches take their candidates from this check.
    pub(crate) fn plant_ground(&self, site: &crate::Site, ground: BlockId, above: BlockId) -> Option<Ground> {
        (site.height >= site.water && site.height + 1 < WORLD_HEIGHT && above.is_air()
            && site.trail <= 0.2 && self.firm[ground.0 as usize] && self.earth[ground.0 as usize])
            .then(|| self.ground(ground))
    }

    /// Whether a block is a small plant.
    pub fn is_plant(&self, block: BlockId) -> bool {
        self.plants[block.0 as usize]
    }

    /// Whether a block holds up what stands on it.
    pub fn is_firm(&self, block: BlockId) -> bool {
        self.firm[block.0 as usize]
    }

    /// Whether snow lies on a surface block: the snow blanket covers what stands on it.
    fn frosty(&self, top: BlockId) -> bool {
        top == self.ids.snow || top == self.ids.powder || top == self.ids.snowy_ground
    }
}

impl Overworld {
    /// The surface rules a site's ground follows and the strata they band with: its biome's,
    /// or those of a terrain shape with ground of its own where the shape stands tallest.
    pub(crate) fn surface_of(&self, site: &Site, x: i32, z: i32) -> (&Surface, &[BlockId]) {
        let biome = self.biomes.get(site.biome);
        match site.form.shape.and_then(|shape| self.shape_surfaces[shape.0 as usize].as_ref()) {
            Some(own) if site.form.strength > 0.3 + 0.3 * self.terrain.dither(x, z) => {
                (&own.surface, if own.strata.is_empty() { &biome.strata } else { &own.strata })
            }
            _ => (&biome.surface, &biome.strata),
        }
    }

    /// The stones beneath a site: its biome's own, or its rock province's.
    pub(crate) fn stones(&self, site: &Site) -> &Palette {
        self.biomes.get(site.biome).stones.as_ref().unwrap_or(&self.rocks.get(site.province).stones)
    }

    /// Bare rock by height, in beds of its rock province's stones; none where the biome
    /// has stones of its own.
    pub(crate) fn rock_bands(&self, site: &Site) -> &[BlockId] {
        match self.biomes.get(site.biome).stones {
            Some(_) => &[],
            None => &self.rocks.get(site.province).bands,
        }
    }

    /// The stones where the rock shows at the surface: the biome's own, or its rock
    /// province's outcrop stones.
    pub(crate) fn outcrop(&self, site: &Site) -> &Palette {
        self.biomes.get(site.biome).stones.as_ref().unwrap_or(&self.rocks.get(site.province).outcrop)
    }

    /// The surface at a site, from the top down. Bare rock's top is the bed at its height,
    /// as in the column below it.
    pub(crate) fn surface_layers(&self, site: &Site, x: i32, z: i32) -> Layers {
        let layers = self.layers_of(site, x, z);
        match layers.filler {
            // Bare rock's top is the bed at its height, unless a ledge holds snow or grass.
            Fill::Rock if layers.top != self.terrain.ids.snow && layers.top != self.terrain.ids.grass => {
                Layers { top: self.terrain.band(self.rock_bands(site), site.height, x, z), ..layers }
            }
            _ => layers,
        }
    }

    fn layers_of(&self, site: &Site, x: i32, z: i32) -> Layers {
        let terrain = &self.terrain;
        let ids = &terrain.ids;
        let d = terrain.dither(x, z);
        let mix = || terrain.mix(x, z);
        let spot = Spot { site, x, z, dither: d, mix: &mix, chance: terrain.chance(x, z), bands: self.rock_bands(site), sampled: Default::default() };
        let biome = self.biomes.get(site.biome);
        let (surface, strata) = self.surface_of(site, x, z);
        let stones = self.outcrop(site);
        let rules = &self.rules;
        // Whether `v` lies past a threshold that is dithered across `lo..hi`.
        let past = |v: f32, lo: f32, hi: f32| v > lo + (hi - lo) * d;
        let t = site.temperature;
        let above = (site.height - SEA_LEVEL) as f32;
        // Height over the sea, lake or river over or beside the site.
        let shore = (site.height - site.water) as f32;

        if shore < 0.0 {
            return rules.underwater(surface, &spot, stones, strata);
        }
        if site.body == Body::Salt {
            // A dry closed basin crusts over with salt.
            return Layers { top: if d < 0.75 { ids.salt } else { ids.sand }, filler: Fill::Block(ids.sand), depth: 3 };
        }
        // Bare rock on cliffs, and on high ground exposed to the weather where no snow lies:
        // judged by the steepness around the position, so a cliff is one patch of rock, with
        // the tallest single steps counted too.
        let cliff = || rules.cliff(surface, &spot, stones, strata);
        let steep = site.steepness.max(0.5 * site.slope as f32);
        if past(steep, 2.3, 3.2) {
            // A ledge in the face, level with its neighbours, holds snow where it is cold
            // and grass where it is not too high, as ledges do.
            let ledge = site.slope == 0 && terrain.chance(x, z) < 0.8;
            if ledge && past(-t, -1.0, 2.0) {
                return Layers { top: ids.snow, ..cliff() };
            }
            if ledge && above < 140.0 && site.vegetation > 0.3 {
                return Layers { top: ids.grass, ..cliff() };
            }
            return cliff();
        }
        // Cold: ground under snow, then deep snow (powdery where it is coldest).
        if past(-t, 1.5, 6.0) {
            let powder = past(-t, 8.0, 12.0) && terrain.patches.get_noise_2d(x as f32, z as f32) > 0.15;
            return Layers { top: if powder { ids.powder } else { ids.snow }, filler: Fill::Block(ids.snow), depth: 2 };
        }
        let exposure = steep + ((above - 95.0) / 12.0).max(0.0);
        if past(exposure, 3.0, 5.5) {
            return cliff();
        }
        // Beaches along the sea. Banks of rivers and lakes are the river biome's own
        // (`Site::biome` takes it near fresh water).
        let beach = site.body == Body::Sea && site.continentalness < 0.7 && !past(above, 0.5, 2.5);
        let mut ground = rules.ground(surface, &spot, beach, stones, strata);
        // Where steps are two blocks high and more, rock shows in the risers under the
        // ground's lip rather than bare soil.
        if past(site.steepness, 1.2, 1.7) && matches!(ground.filler, Fill::Block(block) if terrain.ground(block) == Ground::Soil) {
            let bands = self.rock_bands(site);
            ground = if bands.is_empty() { Layers { filler: Fill::Stones, depth: 0, ..ground } } else { Layers { filler: Fill::Beds, depth: 8, ..ground } };
        }
        // A trail's worn tread, thinning out at its edges into trodden grass.
        if site.trail * 0.95 > d {
            ground.top = terrain.tread(&biome.tags, x, z);
        } else if site.trail > 0.0 && site.trail + 0.4 > d && ground.top == ids.grass {
            ground.top = terrain.worn_edge;
        }
        // Thin snow: the same snowy blocks snow that falls in play makes, else snowy soil.
        if past(-t, -2.5, 1.5) {
            if let Some(snowy) = terrain.snow_cover[ground.top.0 as usize] {
                return Layers { top: snowy, ..ground };
            }
            if terrain.ground(ground.top) == Ground::Soil {
                return Layers { top: ids.snowy_ground, ..ground };
            }
        }
        ground
    }

    /// Top block of dry or submerged ground, and the block (water or ice) above it when
    /// it lies under the sea, a lake or a river.
    pub(crate) fn surface_blocks(&self, site: &Site, x: i32, z: i32) -> (BlockId, Option<BlockId>) {
        let top = self.surface_layers(site, x, z).top;
        let water = (site.height < site.water).then(|| self.terrain.ice(site, x, z).unwrap_or(self.terrain.ids.water));
        (top, water)
    }
}

/// A boulder on a creek's or river's bank, a block or two high, if one lies here: now and
/// then along the water's edge, often beside rapids and plunge pools, mossy where it is
/// damp.
fn boulder(world: &Overworld, site: &Site, geology: &GeoGrid, x: i32, z: i32) -> Option<[Option<BlockId>; 2]> {
    if !matches!(site.body, Body::Creek | Body::River) || site.river < 0.7 || site.height - site.water > 2 {
        return None;
    }
    let fast = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| geology.get(x + dx, z + dz).current.is_some_and(|c| c.speed >= 3));
    // In clumps: a few 3-block cells along a stream hold stones, most of their columns
    // filled, so stones gather in small rocky runs rather than dotting the creek with lone
    // cubes. More of them where the water runs fast.
    let cell = hash3(world.seed ^ 0xb0a2, x.div_euclid(3), 0, z.div_euclid(3));
    if cell % 100 >= if fast { 22 } else { 5 } {
        return None;
    }
    let hash = hash3(world.seed ^ 0xb0a1, x, 0, z);
    if hash % 100 >= 65 {
        return None;
    }
    // Of the local stone, a tall one mossy on top where it is damp.
    let stones = world.outcrop(site);
    let stone = |bits: u64| stones.pick((bits & 0xffff) as f32 / 65536.0);
    let damp = site.soil_moisture() > 0.85 && site.temperature > 0.0;
    let tall = hash >> 16 & 3 == 0;
    let top = if damp && !(hash >> 20).is_multiple_of(3) { world.terrain.ids.mossy_boulder } else { stone(hash >> 24) };
    Some([Some(stone(hash >> 8)), tall.then_some(top)])
}

/// Whether a block is ground or rock (see `Terrain::earth`).
pub(crate) fn earth(registry: &BlockRegistry, block: BlockId) -> bool {
    let def = registry.def(block);
    let wooden = ["_log", "_wood", "_stem", "_roots", "_leaves", "_cap"].iter().any(|s| def.name.ends_with(s)) || def.name.ends_with("cactus");
    def.solid && def.render != voxmar_engine::block::RenderKind::Plant && !wooden && registry.fluid(block).is_none()
}

/// The four neighbours of (x, z).
fn sides_of(geology: &GeoGrid, x: i32, z: i32) -> [crate::geology::GeoSample; 4] {
    [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| geology.get(x + dx, z + dz))
}

/// The band of `strata` at height `y`, shifted by its wobble.
fn stratum(strata: &[BlockId], y: i32) -> BlockId {
    strata[y.rem_euclid(strata.len() as i32) as usize]
}

/// The stone at a block: deepslate in the depths, otherwise one of `stones` in blobs and
/// veins that follow a 3D noise (`rock`, its value here).
fn stone_at(ids: &Ids, stones: &Palette, rock: f32, seed: u64, y: i32, wx: i32, wz: i32) -> BlockId {
    // Deepslate below `DEEPSLATE`, dithered over 8 blocks (hashed only within them).
    if y < DEEPSLATE || y < DEEPSLATE + 7 && y < DEEPSLATE + (hash3(seed ^ 0xd5, wx, y, wz) % 8) as i32 {
        return ids.deepslate;
    }
    // A single octave of simplex noise is spread nearly evenly over about ±0.94.
    stones.pick((0.5 + rock * 0.53).clamp(0.0, 0.9999))
}
