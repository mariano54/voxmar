//! Stage 3: what grows where. Ground cover, tree density and species, and the small
//! plants on the ground are smooth functions of temperature and moisture, so forests thin
//! out gradually into grassland, grassland into scrub and desert, and conifers take over
//! towards the cold. Low-frequency noise adds the patchiness of real vegetation: clearings
//! and groves, birch stands among oaks, meadows, and drifts of one kind of flower.
//!
//! Every species has a climate it likes, so each climate has its own recognisable trees:
//! spruce and pine in the cold, birch where it is cool and wet, oak and big oaks in
//! temperate land, dark oak in warm, very wet forests, cherry on cool mountain slopes,
//! willows along rivers, jungle giants and bushes where it is hot and wet, acacias and
//! shrubs where it is hot and drier, palms on tropical beaches, mangroves in warm
//! shallows, cacti and dead trees in deserts.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::dials::WorldDials;
use crate::geology::{noise, smoothstep};
use crate::rng::hash3;
use crate::trees::Tree;
use crate::{SEA_LEVEL, Site};

/// Chance that a tree grows on a 4×4 candidate cell where the forest is densest.
pub const TREE_CHANCE: f32 = 0.5;
const PLANTS: u64 = 0x91a7;

/// What a surface offers plants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    /// Grass, dirt and other soils.
    Soil,
    /// Snow-covered soil: hardy conifers only.
    Snow,
    /// Sand: cacti, palms, dead bushes.
    Sand,
    /// The floor of shallow water: mangroves.
    Shallows,
    /// Rock, gravel, ice: nothing.
    Bare,
}

/// A small plant: one block, a two-block plant, or a stack of one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plant {
    One(BlockId),
    Tall(BlockId, BlockId),
    Stack(BlockId, i32),
}

/// The temperature (°C) and moisture ranges a flower likes, and how common it is there.
struct Flower {
    block: BlockId,
    temperature: (f32, f32),
    moisture: (f32, f32),
    weight: f32,
}

/// 1 between `inner`, 0 outside `outer`, smooth in between.
fn band(v: f32, outer: (f32, f32), inner: (f32, f32)) -> f32 {
    smoothstep(outer.0, inner.0, v) * (1.0 - smoothstep(inner.1, outer.1, v))
}

pub struct Ecology {
    grass: BlockId,
    tall_grass: (BlockId, BlockId),
    fern: BlockId,
    large_fern: (BlockId, BlockId),
    dead_bush: BlockId,
    sweet_berries: BlockId,
    sugar_cane: BlockId,
    mushrooms: [BlockId; 2],
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
            tall_grass: (id("tall_grass"), id("tall_grass_top")),
            fern: id("fern"),
            large_fern: (id("large_fern"), id("large_fern_top")),
            dead_bush: id("dead_bush"),
            sweet_berries: id("sweet_berry_bush"),
            sugar_cane: id("sugar_cane"),
            mushrooms: [id("brown_mushroom"), id("red_mushroom")],
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

    /// The chance, per candidate cell, that each kind of tree grows here (in `Tree::ALL`
    /// order). With `ground` unknown, ground does not restrict the kinds, which gives an
    /// upper bound.
    pub fn tree_chances(&self, site: &Site, ground: Option<Ground>, x: i32, z: i32) -> [f32; Tree::ALL.len()] {
        let (t, m) = (site.temperature, site.soil_moisture());
        let above = site.height - SEA_LEVEL;
        // Height over the sea, lake or river over or beside the site.
        let shore = site.height - site.water;
        let grove = smoothstep(-0.3, 0.3, self.groves.get_noise_2d(x as f32, z as f32));
        let mut chances = [0.0; Tree::ALL.len()];
        let mut set = |tree: Tree, chance: f32| chances[tree as usize] += chance;

        if shore >= 1 {
            // Forest trees share the forest by how well each suits the climate.
            let shares = [
                (Tree::Oak, band(t, (5.0, 25.0), (9.0, 21.0)) * (1.5 - grove)),
                (Tree::BigOak, band(t, (8.0, 22.0), (11.0, 19.0)) * smoothstep(0.9, 1.3, m) * 0.15),
                (Tree::Birch, band(t, (1.0, 15.0), (5.0, 11.0)) * smoothstep(0.7, 1.1, m) * (0.2 + 1.6 * grove)),
                (Tree::Spruce, (1.0 - smoothstep(3.0, 10.0, t)) * smoothstep(0.75, 1.1, m)),
                (Tree::GiantSpruce, band(t, (-4.0, 9.0), (0.0, 3.0)) * smoothstep(1.2, 1.6, m) * 0.3),
                (Tree::Pine, (1.0 - smoothstep(5.0, 12.0, t)) * (1.0 - smoothstep(0.85, 1.15, m))),
                (Tree::DarkOak, band(t, (11.0, 23.0), (15.0, 19.0)) * smoothstep(1.15, 1.45, m) * (0.1 + 1.8 * (1.0 - grove))),
                (Tree::Cherry, band(t, (4.0, 16.0), (8.0, 13.0)) * smoothstep(0.7, 1.0, m) * smoothstep(30.0, 55.0, above as f32) * 1.5),
                (Tree::Jungle, smoothstep(19.0, 24.0, t) * smoothstep(1.0, 1.3, m)),
                (Tree::GiantJungle, smoothstep(21.0, 25.0, t) * smoothstep(1.3, 1.6, m) * 0.4),
                (Tree::JungleBush, smoothstep(19.0, 24.0, t) * smoothstep(0.9, 1.2, m) * 0.9),
                (Tree::Acacia, smoothstep(16.0, 21.0, t) * (1.0 - smoothstep(0.75, 1.05, m))),
            ];
            let total: f32 = shares.iter().map(|(_, share)| share).sum();
            if total > 0.0 {
                let forest = site.forest * TREE_CHANCE;
                for (tree, share) in shares {
                    set(tree, forest * share / total);
                }
            }
            // Other woody plants grow on their own terms.
            set(Tree::Willow, 0.15 * smoothstep(0.25, 0.6, site.river) * band(t, (6.0, 26.0), (10.0, 22.0)));
            set(Tree::Shrub, 0.07 * band(m, (0.2, 1.0), (0.4, 0.8)) * smoothstep(6.0, 12.0, t) + 0.03 * site.forest);
            let dry_dead = smoothstep(14.0, 18.0, t) * band(m, (0.15, 0.5), (0.25, 0.4));
            let cold_dead = band(t, (-10.0, -2.0), (-8.0, -4.0)) * smoothstep(0.3, 0.6, m);
            set(Tree::Dead, 0.012 * (dry_dead + cold_dead));
            set(Tree::Cactus, smoothstep(18.0, 24.0, t) * (1.0 - smoothstep(0.12, 0.3, m)) * 0.06);
            let beach = shore <= 3 && (site.continentalness < 0.74 || site.river > 0.5);
            set(Tree::Palm, if beach { 0.12 * smoothstep(20.0, 24.0, t) * smoothstep(0.4, 0.7, m) } else { 0.0 });
        }
        let shallows = (-2..=1).contains(&shore) && (site.continentalness < 0.74 || site.river > 0.3);
        set(Tree::Mangrove, if shallows { 0.35 * smoothstep(21.0, 25.0, t) * smoothstep(0.95, 1.25, m) } else { 0.0 });

        if let Some(ground) = ground {
            for (tree, chance) in Tree::ALL.into_iter().zip(&mut chances) {
                if !grows_on(tree, ground) {
                    *chance = 0.0;
                }
            }
        }
        chances
    }

    /// The tree that dominates the canopy at a site, and the fraction of ground under
    /// crowns, for distant terrain. Shrubs and cacti make no canopy.
    pub fn canopy(&self, site: &Site, ground: Ground, x: i32, z: i32) -> Option<(Tree, f32)> {
        let crowns = Tree::ALL
            .into_iter()
            .zip(self.tree_chances(site, Some(ground), x, z))
            .filter(|(tree, chance)| *chance > 0.0 && !matches!(tree, Tree::Shrub | Tree::JungleBush | Tree::Cactus | Tree::Dead));
        let (total, dominant) = crowns.fold((0.0, None::<(Tree, f32)>), |(total, best), (tree, chance)| {
            (total + chance, if best.is_some_and(|(_, b)| b >= chance) { best } else { Some((tree, chance)) })
        });
        // A crown covers about six candidate cells' worth of ground; crowns overlap at random.
        dominant.map(|(tree, _)| (tree, 1.0 - (-1.5 * total).exp()))
    }

    /// The small plant growing on `ground` at a site, if any. Each position decides alone,
    /// from a hash and smooth noise, so plants never depend on generation order.
    pub fn plant(&self, site: &Site, ground: Ground, x: i32, z: i32) -> Option<Plant> {
        let hash = hash3(self.seed ^ PLANTS, x, 0, z);
        let unit = |bits: u64| (bits & 0xff_ffff) as f32 / (1u32 << 24) as f32;
        let (second, third) = (unit(hash >> 24), unit(hash >> 40));
        let (t, m) = (site.temperature, site.soil_moisture());
        let shore = site.height - site.water;
        let bank = smoothstep(0.35, 0.7, site.river) * smoothstep(10.0, 14.0, t) * if shore <= 3 { 1.0 } else { 0.0 };
        // Each kind of plant takes a slice of one roll, as wide as its chance here.
        let mut roll = unit(hash);
        let mut take = |chance: f32| {
            let hit = roll < chance;
            roll -= chance;
            hit
        };
        match ground {
            Ground::Soil => {
                let meadow = 0.45 + 0.55 * smoothstep(-0.5, 0.4, self.meadows.get_noise_2d(x as f32, z as f32));
                if take(0.6 * site.vegetation * meadow * (1.0 - 0.4 * site.forest)) {
                    // Ferns in wet forests, in the cool north and the tropics; tall grass
                    // where it is warm and lush.
                    let climate = (1.0 - smoothstep(5.0, 12.0, t)).max(smoothstep(18.0, 23.0, t));
                    let ferns = smoothstep(0.8, 1.4, m) * (0.1 + 0.6 * site.forest) * climate;
                    let tall = 0.1 + 0.3 * smoothstep(0.9, 1.5, m) * band(t, (6.0, 30.0), (12.0, 24.0));
                    return Some(match (second < ferns, third < tall) {
                        (true, true) => Plant::Tall(self.large_fern.0, self.large_fern.1),
                        (true, false) => Plant::One(self.fern),
                        (false, true) => Plant::Tall(self.tall_grass.0, self.tall_grass.1),
                        (false, false) => Plant::One(self.grass),
                    });
                }
                if take(0.012 * smoothstep(0.45, 0.75, site.forest) * smoothstep(0.9, 1.2, m) * band(t, (2.0, 26.0), (6.0, 22.0))) {
                    return Some(Plant::One(self.mushrooms[(second < 0.4) as usize]));
                }
                if take(0.01 * smoothstep(0.25, 0.5, site.forest) * band(t, (-4.0, 10.0), (0.0, 6.0))) {
                    return Some(Plant::One(self.sweet_berries));
                }
                if take(0.06 * bank) {
                    return Some(Plant::Stack(self.sugar_cane, 1 + (second * 3.0) as i32));
                }
                if take(0.02 * smoothstep(14.0, 20.0, t) * (1.0 - smoothstep(0.3, 0.55, m))) {
                    return Some(Plant::One(self.dead_bush));
                }
                let climate = band(t, (3.0, 31.0), (8.0, 27.0)) * smoothstep(0.45, 0.85, m);
                let field = 0.04 + smoothstep(0.1, 0.55, self.blooms.get_noise_2d(x as f32, z as f32));
                let flowers = 0.22 * climate * field * (1.0 - 0.6 * site.forest) * (0.6 + 0.8 * self.richness);
                take(flowers).then(|| Plant::One(self.flower(site, x, z, second)))
            }
            Ground::Sand => {
                if take(0.04 * bank) {
                    return Some(Plant::Stack(self.sugar_cane, 1 + (second * 3.0) as i32));
                }
                let dead = smoothstep(15.0, 21.0, t) * (1.0 - smoothstep(0.15, 0.4, m)) * 0.03;
                take(dead).then_some(Plant::One(self.dead_bush))
            }
            Ground::Snow | Ground::Shallows | Ground::Bare => None,
        }
    }

    /// Picks a flower suited to the site. A smooth noise chooses along the cumulative
    /// weights, so neighbours mostly share a kind and flowers grow in drifts.
    fn flower(&self, site: &Site, x: i32, z: i32, jitter: f32) -> BlockId {
        let (t, m) = (site.temperature, site.soil_moisture());
        let fit = |v: f32, (lo, hi): (f32, f32), margin: f32| band(v, (lo - margin, hi + margin), (lo + margin, hi - margin));
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

/// Which grounds a kind of tree takes root in.
fn grows_on(tree: Tree, ground: Ground) -> bool {
    match tree {
        Tree::Mangrove => matches!(ground, Ground::Shallows | Ground::Soil | Ground::Sand),
        Tree::Cactus => ground == Ground::Sand,
        Tree::Palm => matches!(ground, Ground::Sand | Ground::Soil),
        Tree::Spruce | Tree::GiantSpruce | Tree::Pine | Tree::Dead => matches!(ground, Ground::Soil | Ground::Snow),
        _ => ground == Ground::Soil,
    }
}

/// Ground cover (0..=1) the climate supports: grass needs less water than trees and
/// tolerates more cold.
pub fn ground_cover(temperature: f32, water: f32) -> f32 {
    smoothstep(0.1, 0.55, water) * smoothstep(-6.0, 4.0, temperature)
}
