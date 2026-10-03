//! Stage 3: what grows where. Ground cover, tree density and species, and the small
//! plants on the ground are smooth functions of temperature and moisture, so forests thin
//! out gradually into grassland, grassland into scrub and desert, and conifers take over
//! towards the cold. Low-frequency noise adds the patchiness of real vegetation: clearings
//! and groves, birch stands among oaks, meadows, and drifts of one kind of flower.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::Site;
use crate::dials::WorldDials;
use crate::geology::{noise, smoothstep};
use crate::rng::{Rng, hash3};

/// Chance that a tree grows on a 4×4 candidate cell where the forest is densest.
pub const TREE_CHANCE: f32 = 0.5;
const PLANTS: u64 = 0x91a7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tree {
    Oak,
    Birch,
    Spruce,
    Jungle,
    Acacia,
}

impl Tree {
    pub const ALL: [Tree; 5] = [Tree::Oak, Tree::Birch, Tree::Spruce, Tree::Jungle, Tree::Acacia];

    pub fn name(self) -> &'static str {
        match self {
            Tree::Oak => "oak",
            Tree::Birch => "birch",
            Tree::Spruce => "spruce",
            Tree::Jungle => "jungle",
            Tree::Acacia => "acacia",
        }
    }
}

/// What a surface block offers plants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    /// Grass, dirt and other soils: trees, grass and flowers.
    Soil,
    /// Sand: cacti and dead bushes where it is hot and dry.
    Sand,
    /// Rock, snow, gravel, ice: nothing.
    Bare,
}

/// The temperature (°C) and moisture ranges a flower likes, and how common it is there.
struct Flower {
    block: BlockId,
    temperature: (f32, f32),
    moisture: (f32, f32),
    weight: f32,
}

pub struct Ecology {
    grass: BlockId,
    fern: BlockId,
    dead_bush: BlockId,
    flowers: Vec<Flower>,
    /// Clearings and groves in forests.
    patches: FastNoiseLite,
    /// Stands of one tree species.
    groves: FastNoiseLite,
    /// Lush and sparse grass.
    meadows: FastNoiseLite,
    /// Flower fields.
    blooms: FastNoiseLite,
    /// Which flower grows in a drift.
    kinds: FastNoiseLite,
    richness: f32,
    seed: u64,
}

impl Ecology {
    pub fn new(seed: u64, dials: &WorldDials, registry: &BlockRegistry) -> Self {
        let id = |name: &str| registry.expect(&format!("base:{name}"));
        let flower = |name: &str, temperature, moisture, weight| Flower { block: id(name), temperature, moisture, weight };
        Self {
            grass: id("short_grass"),
            fern: id("fern"),
            dead_bush: id("dead_bush"),
            flowers: vec![
                flower("dandelion", (4.0, 26.0), (0.4, 1.6), 1.0),
                flower("poppy", (5.0, 26.0), (0.4, 1.6), 1.0),
                flower("cornflower", (8.0, 20.0), (0.5, 1.2), 0.6),
                flower("oxeye_daisy", (6.0, 20.0), (0.6, 1.4), 0.6),
                flower("azure_bluet", (8.0, 22.0), (0.5, 1.3), 0.6),
                flower("allium", (3.0, 14.0), (0.6, 1.3), 0.5),
                flower("lily_of_the_valley", (2.0, 13.0), (1.0, 2.5), 0.6),
                flower("blue_orchid", (18.0, 32.0), (1.15, 3.0), 0.9),
                flower("red_tulip", (9.0, 18.0), (0.5, 1.0), 0.3),
                flower("orange_tulip", (9.0, 18.0), (0.5, 1.0), 0.3),
                flower("white_tulip", (9.0, 18.0), (0.5, 1.0), 0.3),
                flower("pink_tulip", (9.0, 18.0), (0.5, 1.0), 0.3),
            ],
            patches: noise(seed, 30, 1.0 / 300.0, FractalType::FBm, 2),
            groves: noise(seed, 31, 1.0 / 140.0, FractalType::FBm, 2),
            meadows: noise(seed, 32, 1.0 / 40.0, FractalType::FBm, 2),
            blooms: noise(seed, 33, 1.0 / 90.0, FractalType::FBm, 2),
            kinds: noise(seed, 34, 1.0 / 22.0, FractalType::None, 1),
            richness: dials.vegetation,
            seed,
        }
    }

    /// Tree cover (0..=1) at a position with this temperature and soil moisture: what the
    /// climate supports, broken into clearings and groves.
    pub fn forest(&self, x: i32, z: i32, temperature: f32, water: f32) -> f32 {
        let climate = smoothstep(0.45, 1.2, water) * smoothstep(-7.0, 2.0, temperature);
        if climate <= 0.0 {
            return 0.0;
        }
        let patch = 0.08 + 0.92 * smoothstep(-0.2, 0.5, self.patches.get_noise_2d(x as f32, z as f32));
        (climate * patch * (0.6 + 0.5 * self.richness)).min(1.0)
    }

    /// How well each species does here, in `Tree::ALL` order: spruce in the cold, birch
    /// where it is cool and wet, oak in temperate land, jungle trees where it is hot and
    /// wet, acacia where it is hot and drier. Groves tip the balance locally.
    fn tree_weights(&self, site: &Site, x: i32, z: i32) -> [f32; 5] {
        let (t, m) = (site.temperature, site.soil_moisture());
        let grove = smoothstep(-0.3, 0.3, self.groves.get_noise_2d(x as f32, z as f32));
        let oak = smoothstep(6.0, 10.0, t) * (1.0 - 0.8 * smoothstep(21.0, 25.0, t)) * (1.6 - 1.2 * grove);
        let birch = smoothstep(2.0, 6.0, t) * (1.0 - smoothstep(11.0, 15.0, t)) * smoothstep(0.7, 1.1, m) * (0.2 + 1.6 * grove);
        let spruce = 1.0 - smoothstep(2.0, 9.0, t);
        let jungle = smoothstep(19.0, 24.0, t) * smoothstep(0.95, 1.3, m);
        let acacia = smoothstep(17.0, 22.0, t) * (1.0 - smoothstep(0.75, 1.05, m));
        [oak, birch, spruce, jungle, acacia]
    }

    /// Picks the species of a tree growing at a site.
    pub fn tree(&self, site: &Site, x: i32, z: i32, rng: &mut Rng) -> Tree {
        let weights = self.tree_weights(site, x, z);
        let mut roll = rng.unit() * weights.iter().sum::<f32>();
        for (tree, weight) in Tree::ALL.into_iter().zip(weights) {
            if roll < weight {
                return tree;
            }
            roll -= weight;
        }
        Tree::Oak
    }

    /// The species that dominates the canopy at a site.
    pub fn dominant_tree(&self, site: &Site, x: i32, z: i32) -> Tree {
        let weights = self.tree_weights(site, x, z);
        Tree::ALL.into_iter().zip(weights).max_by(|a, b| a.1.total_cmp(&b.1)).map_or(Tree::Oak, |(tree, _)| tree)
    }

    /// Chance that a cactus grows on a candidate cell: hot, dry sand.
    pub fn cactus_chance(site: &Site) -> f32 {
        smoothstep(18.0, 24.0, site.temperature) * (1.0 - smoothstep(0.12, 0.3, site.soil_moisture())) * 0.06
    }

    /// The small plant growing on `ground` at a site, if any. Each position decides alone,
    /// from a hash and smooth noise, so plants never depend on generation order.
    pub fn plant(&self, site: &Site, ground: Ground, x: i32, z: i32) -> Option<BlockId> {
        let hash = hash3(self.seed ^ PLANTS, x, 0, z);
        let unit = |bits: u64| (bits & 0xff_ffff) as f32 / (1u32 << 24) as f32;
        let mut roll = unit(hash);
        let (t, m) = (site.temperature, site.soil_moisture());
        match ground {
            Ground::Soil => {
                let meadow = 0.45 + 0.55 * smoothstep(-0.5, 0.4, self.meadows.get_noise_2d(x as f32, z as f32));
                let grass = 0.6 * site.vegetation * meadow * (1.0 - 0.4 * site.forest);
                if roll < grass {
                    // Ferns in wet forests, in the cool north and the tropics.
                    let climate = (1.0 - smoothstep(5.0, 12.0, t)).max(smoothstep(18.0, 23.0, t));
                    let ferns = smoothstep(0.8, 1.4, m) * (0.1 + 0.6 * site.forest) * climate;
                    return Some(if unit(hash >> 24) < ferns { self.fern } else { self.grass });
                }
                roll -= grass;
                let climate = smoothstep(3.0, 8.0, t) * (1.0 - smoothstep(27.0, 31.0, t)) * smoothstep(0.45, 0.85, m);
                let field = 0.04 + smoothstep(0.1, 0.55, self.blooms.get_noise_2d(x as f32, z as f32));
                let flowers = 0.22 * climate * field * (1.0 - 0.6 * site.forest) * (0.6 + 0.8 * self.richness);
                (roll < flowers).then(|| self.flower(site, x, z, unit(hash >> 24)))
            }
            Ground::Sand => {
                let dead = smoothstep(15.0, 21.0, t) * (1.0 - smoothstep(0.15, 0.4, m)) * 0.03;
                (roll < dead).then_some(self.dead_bush)
            }
            Ground::Bare => None,
        }
    }

    /// Picks a flower suited to the site. A smooth noise chooses along the cumulative
    /// weights, so neighbours mostly share a kind and flowers grow in drifts.
    fn flower(&self, site: &Site, x: i32, z: i32, jitter: f32) -> BlockId {
        let (t, m) = (site.temperature, site.soil_moisture());
        let fit = |v: f32, (lo, hi): (f32, f32), margin: f32| {
            smoothstep(lo - margin, lo + margin, v) * (1.0 - smoothstep(hi - margin, hi + margin, v))
        };
        let weight = |f: &Flower| f.weight * fit(t, f.temperature, 2.0) * fit(m, f.moisture, 0.15);
        let total: f32 = self.flowers.iter().map(weight).sum();
        let drift = self.kinds.get_noise_2d(x as f32, z as f32) * 2.0 + 0.5 + 0.15 * (jitter - 0.5);
        let mut roll = drift.rem_euclid(1.0) * total;
        for flower in &self.flowers {
            let weight = weight(flower);
            if roll < weight {
                return flower.block;
            }
            roll -= weight;
        }
        self.flowers[0].block
    }
}

/// Ground cover (0..=1) the climate supports: grass needs less water than trees and
/// tolerates more cold.
pub fn ground_cover(temperature: f32, water: f32) -> f32 {
    smoothstep(0.1, 0.55, water) * smoothstep(-6.0, 4.0, temperature)
}
