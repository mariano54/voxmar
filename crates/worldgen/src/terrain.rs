//! Stage 4: turns sites and resources into blocks: surface materials, caves (see
//! [`crate::caves`]), water and ice at the level of the sea, lake or river over each
//! column, then trees and the small plants on the ground.
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
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

use crate::biome::Biomes;
use crate::caves::Cave;
use crate::content::WorldContent;
use crate::ecology::{Ground, Plant};
use crate::features::{self, Features};
use crate::geology::{GeoGrid, noise};
use crate::lattice::{self, Lattice, Profile};
use crate::resources::Resources;
use crate::rng::hash3;
use crate::rocks::Rocks;
use crate::surface::{Fill, Layers, Palette, Spot};
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
    boulder: BlockId,
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
    rock: FastNoiseLite,
    /// Patches that dither the borders between surface materials.
    patches: FastNoiseLite,
    /// Larger patches for choosing among mixed blocks.
    mixes: FastNoiseLite,
    /// Whether wood may replace a block: air, water and leaves, by block id.
    soft: Vec<bool>,
    /// Whether ore may replace a block: every stone, by block id.
    stone: Vec<bool>,
    /// What each block offers plants, by block id.
    grounds: Vec<Ground>,
    seed: u64,
    resources: Resources,
    features: Features,
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
        let soft = by_id(&|block| block.is_air() || block == water || registry.def(block).name.ends_with("_leaves"));
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
        Self {
            ids: Ids {
                bedrock: id("bedrock"),
                deepslate: id("deepslate"),
                grass: id("grass_block"),
                water,
                lava: id("lava"),
                salt: id("calcite"),
                boulder: id("cobblestone"),
                mossy_boulder: id("mossy_cobblestone"),
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
            stone: by_id(&|block| stones.contains(&block)),
            grounds,
            seed,
            resources: Resources::new(registry),
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

    fn get_local(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.data[Self::local(x, y, z)]
    }

    fn set_local(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
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

    /// Wood grows through air, water and leaves; whether it did.
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

    // Geology for the column plus a one-block border (for slopes).
    let geology = GeoGrid::new(world, (origin.x - 1, origin.z - 1), CHUNK_SIZE + 2);
    let top = geology.samples().iter().map(|g| g.height.max(g.water)).max().unwrap_or(0).max(SEA_LEVEL);

    let rock = Lattice::noise(&terrain.rock, origin, 0, top, lattice::STEP, 1.0);
    let mut rock_profile = Profile::default();
    let mut caves = world.caves.column(&geology, origin, top);
    let underground = world.underground.column(world, &area, &geology, origin, top);
    let mut cave = Vec::with_capacity(WORLD_HEIGHT as usize);

    let mut blocks = Blocks {
        origin,
        data: vec![BlockId::AIR; CHUNK_VOLUME * COLUMN_CHUNKS as usize],
        stone: &terrain.stone,
        soft: &terrain.soft,
    };
    let mut column = Column::default();
    let mut surfaces = Vec::with_capacity((CHUNK_SIZE * CHUNK_SIZE) as usize);

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (wx, wz) = (origin.x + x, origin.z + z);
            let geo = geology.get(wx, wz);
            let height = geo.height;
            let site = area.site_on(&geo, wx, wz, geology.slope(wx, wz));
            column.climate[(z * CHUNK_SIZE + x) as usize] = site.tint();
            let colors = area.colors_of(&site, wx, wz);
            column.colors[(z * CHUNK_SIZE + x) as usize] = [colors.grass, colors.foliage, colors.water];
            let layers = world.surface_layers(&site, wx, wz);
            let stones = world.stones(&site);
            let strata = &world.biomes.get(site.biome).strata;
            caves.carve(x, z, &mut cave);
            rock.profile(x, z, &mut rock_profile);
            let rocky = underground.rocky(x, z);

            for y in 0..=height {
                let below_surface = height - y;
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
                let block = if y == 0 || (y < 4 && hash3(world.seed, wx, y, wz) % 4 >= y as u64) {
                    ids.bedrock
                } else if below_surface == 0 {
                    layers.top
                } else if below_surface <= layers.depth && layers.filler != Fill::Stones {
                    match layers.filler {
                        Fill::Block(block) => block,
                        _ => terrain.band(strata, y, wx, wz),
                    }
                } else {
                    let rock = rock_profile.at(y);
                    match underground.rock(x, y, z).filter(|_| rocky) {
                        Some(Rock::Stones(stones)) => stones.pick((0.5 + rock * 0.53).clamp(0.0, 0.9999)),
                        Some(Rock::Strata(strata)) => terrain.band(strata, y, wx, wz),
                        None => stone_at(ids, stones, rock, world.seed, y, wx, wz),
                    }
                };
                blocks.set_local(x, y, z, block);
            }
            underground.decorate(x, z, &site, &cave, &mut blocks);
            underground.bury(x, z, &cave, &mut blocks);
            // Fast water stays open in the cold.
            let ice = terrain.ice(&site, wx, wz).filter(|_| geo.current.is_none_or(|c| c.speed < 3) && geo.body != Body::Spring);
            for y in height + 1..=geo.water {
                blocks.set_local(x, y, z, if y == geo.water { ice.unwrap_or(ids.water) } else { ids.water });
            }
            // Rocks break the surface of rapids, though not right under a fall, whose water
            // must land in water.
            if geo.current.is_some_and(|c| c.speed >= 3)
                && matches!(geo.body, Body::Creek | Body::River)
                && hash3(world.seed ^ 0xb01d, wx, 0, wz) % 100 < 9
                && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().all(|&(dx, dz)| {
                    let n = geology.get(wx + dx, wz + dz);
                    !n.submerged() || n.water <= geo.water
                })
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
                if let Some(deck) = trail.deck.filter(|&d| d > height && d < WORLD_HEIGHT) {
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
                    blocks.set_local(x, y, z, if y <= geo.water { ids.water } else { BlockId::AIR });
                }
            }
            surfaces.push((site, layers.top));
        }
    }

    terrain.resources.place(world.seed, pos, &mut blocks);
    terrain.features.place(world, &area, &geology, &caves, pos, &mut blocks);
    terrain.markers(world, &area, &geology, pos, &mut blocks);

    // Ground plants last, wherever trees, caves and water left the surface open.
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (site, surface) = &surfaces[(z * CHUNK_SIZE + x) as usize];
            let h = site.height;
            let open = |y: i32| y < WORLD_HEIGHT && blocks.get_local(x, y, z).is_air();
            if h < site.water || blocks.get_local(x, h, z) != *surface || !open(h + 1) || site.trail > 0.2 {
                continue;
            }
            let (wx, wz) = (origin.x + x, origin.z + z);
            if let Some(boulder) = boulder(world, site, &geology, wx, wz) {
                for (y, block) in (h + 1..).zip(boulder.into_iter().flatten()) {
                    if y >= WORLD_HEIGHT || !blocks.get_local(x, y, z).is_air() {
                        break;
                    }
                    blocks.set_local(x, y, z, block);
                }
                continue;
            }
            match world.ecology.plant(site, terrain.ground(*surface), origin.x + x, origin.z + z) {
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

    for (cy, chunk) in column.chunks.iter_mut().enumerate() {
        *chunk = Chunk::from_dense(&blocks.data[cy * CHUNK_VOLUME..(cy + 1) * CHUNK_VOLUME]);
    }
    column
}

impl Terrain {
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
                    let site = area.site_on(&geo, at.x, at.y, 0);
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

    /// A uniform-ish 0..1 value for dithering thresholds: smooth patches plus per-block
    /// speckle, so materials interleave in clumps and scattered blocks across a band.
    pub(crate) fn dither(&self, x: i32, z: i32) -> f32 {
        let speckle = (hash3(self.seed ^ 0x5fac, x, 0, z) & 0xffff) as f32 / 65536.0;
        let patch = self.patches.get_noise_2d(x as f32, z as f32) * 0.6 + 0.5;
        (0.55 * patch + 0.45 * speckle).clamp(0.0, 1.0)
    }

    /// Uniform per-column randomness for the surface rules.
    pub(crate) fn chance(&self, x: i32, z: i32) -> f32 {
        (hash3(self.seed ^ 0xc4a, x, 0, z) & 0xffff) as f32 / 65536.0
    }

    /// Another clumpy value in 0..1, independent of `dither`, for choosing among mixed
    /// blocks so that each forms patches with speckled edges.
    pub(crate) fn mix(&self, x: i32, z: i32) -> f32 {
        let speckle = (hash3(self.seed ^ 0x3a1f, x, 0, z) & 0xffff) as f32 / 65536.0;
        let patch = crate::climate::uniform(self.mixes.get_noise_2d(x as f32, z as f32));
        (0.8 * patch + 0.2 * speckle).clamp(0.0, 0.9999)
    }

    /// Strata: bands of rock, one block each, that wobble gently.
    fn band(&self, strata: &[BlockId], y: i32, x: i32, z: i32) -> BlockId {
        let wobble = (self.patches.get_noise_2d(x as f32 * 0.25, z as f32 * 0.25) * 2.5).round() as i32;
        strata[(y + wobble).rem_euclid(strata.len() as i32) as usize]
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
}

impl Overworld {
    /// The stones beneath a site: its biome's own, or its rock province's.
    pub(crate) fn stones(&self, site: &Site) -> &Palette {
        self.biomes.get(site.biome).stones.as_ref().unwrap_or(&self.rocks.get(site.province).stones)
    }

    /// The surface at a site, from the top down.
    pub(crate) fn surface_layers(&self, site: &Site, x: i32, z: i32) -> Layers {
        let terrain = &self.terrain;
        let ids = &terrain.ids;
        let biome = self.biomes.get(site.biome);
        let d = terrain.dither(x, z);
        let mix = || terrain.mix(x, z);
        let spot = Spot { site, x, z, dither: d, mix: &mix, chance: terrain.chance(x, z) };
        let (stones, strata) = (self.stones(site), &biome.strata);
        let rules = &self.rules;
        // Whether `v` lies past a threshold that is dithered across `lo..hi`.
        let past = |v: f32, lo: f32, hi: f32| v > lo + (hi - lo) * d;
        let t = site.temperature;
        let above = (site.height - SEA_LEVEL) as f32;
        // Height over the sea, lake or river over or beside the site.
        let shore = (site.height - site.water) as f32;

        if shore < 0.0 {
            return rules.underwater(&biome.surface, &spot, stones, strata);
        }
        if site.body == Body::Salt {
            // A dry closed basin crusts over with salt.
            return Layers { top: if d < 0.75 { ids.salt } else { ids.sand }, filler: Fill::Block(ids.sand), depth: 3 };
        }
        // Bare rock on cliffs, and on high ground exposed to the weather where no snow lies.
        let cliff = || rules.cliff(&biome.surface, &spot, stones, strata);
        if past(site.slope as f32, 3.0, 5.5) {
            return cliff();
        }
        // Cold: ground under snow, then deep snow (powdery where it is coldest).
        if past(-t, 1.5, 6.0) {
            let powder = past(-t, 8.0, 12.0) && terrain.patches.get_noise_2d(x as f32, z as f32) > 0.15;
            return Layers { top: if powder { ids.powder } else { ids.snow }, filler: Fill::Block(ids.snow), depth: 2 };
        }
        let exposure = site.slope as f32 + ((above - 95.0) / 12.0).max(0.0);
        if past(exposure, 3.0, 5.5) {
            return cliff();
        }
        // Beaches along the sea. Banks of rivers and lakes are the river biome's own
        // (`Site::biome` takes it near fresh water).
        let beach = site.body == Body::Sea && site.continentalness < 0.7 && !past(above, 0.5, 2.5);
        let mut ground = rules.ground(&biome.surface, &spot, beach, stones, strata);
        // A trail's worn tread, thinning out at its edges into trodden grass.
        if site.trail * 0.95 > d {
            ground.top = terrain.tread(&biome.tags, x, z);
        } else if site.trail > 0.0 && site.trail + 0.4 > d && ground.top == ids.grass {
            ground.top = terrain.worn_edge;
        }
        if past(-t, -2.5, 1.5) && terrain.ground(ground.top) == Ground::Soil {
            return Layers { top: ids.snowy_ground, ..ground };
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
    let hash = hash3(world.seed ^ 0xb0a1, x, 0, z);
    let chance = if fast { 18 } else { 3 };
    if hash % 100 >= chance {
        return None;
    }
    let ids = &world.terrain.ids;
    let damp = site.soil_moisture() > 0.85 && site.temperature > 0.0;
    let stone = |bits: u64| if damp && !bits.is_multiple_of(3) { ids.mossy_boulder } else { ids.boulder };
    Some([Some(stone(hash >> 8)), (hash >> 16 & 3 == 0).then(|| stone(hash >> 20))])
}

/// The stone at a block: deepslate in the depths, otherwise one of `stones` in blobs and
/// veins that follow a 3D noise (`rock`, its value here).
fn stone_at(ids: &Ids, stones: &Palette, rock: f32, seed: u64, y: i32, wx: i32, wz: i32) -> BlockId {
    // Deepslate below `DEEPSLATE`, dithered over 8 blocks.
    if y < DEEPSLATE + (hash3(seed ^ 0xd5, wx, y, wz) % 8) as i32 {
        return ids.deepslate;
    }
    // A single octave of simplex noise is spread nearly evenly over about ±0.94.
    stones.pick((0.5 + rock * 0.53).clamp(0.0, 0.9999))
}
