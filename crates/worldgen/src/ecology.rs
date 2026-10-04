//! Stage 3: what grows where. Trees and plants are a shared pool defined as data
//! (`trees.ron`, `plants.ron`); each biome selects from the pool by tag or name, and at
//! every spot a candidate's weight is the biome's weight for it times how well the
//! climate there suits it, times its rarity. So the same pool varies smoothly inside a
//! biome and across biomes: oaks give way to birches as it gets cooler, and a mod's new
//! flower grows wherever its tags and climate fit.
//!
//! Low-frequency noise adds the patchiness of real vegetation. One field, the stand, grades
//! a forest's structure from old-growth cores (dense, tall, with the giants) through
//! ordinary woodland to scrubby edges and gaps (Terralith's band-pass forests), so tree
//! density and tree size change together. A second, global field opens clearings with a
//! soft edge across every biome: trees fade out first, shrubs reach further in, and meadow
//! plants and the clearing's own decorations fill them (Wilder Wild's clearings). Further
//! noise makes stands of one tree species among others, lusher and sparser meadows, and
//! drifts of one kind of flower. Where aether runs high, rare fantastical trees and plants
//! turn up.
//!
//! Small plants are decided block by block in a fixed order of stages: flowers (and
//! wonders), fungi and other plants that grow alone, then the grasses and ferns that
//! carpet whatever is left, so most open ground in a meadow carries a plant.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::biome::BiomeId;
use crate::content::{Comfort, FloraDef, Habitat, PlantDef, Select, Spread, TreeDef, WorldContent};
use crate::dials::WorldDials;
use crate::geology::{noise, smoothstep};
use crate::rarity::{Rarity, Tier};
use crate::rng::hash3;
use crate::trees::TreeId;
use crate::Site;

pub use crate::content::Ground;

/// Chance that a tree grows on a 4×4 candidate cell where the forest is densest.
pub const TREE_CHANCE: f32 = 0.6;
/// Where the clearing field (evenly spread over 0 ..= 1) starts to open a clearing, and
/// where the clearing is fully open: about a tenth of the land lies in clearings.
const CLEARING: (f32, f32) = (0.85, 0.95);
/// Flowers where the bloom field (evenly spread over 0 ..= 1) is low: a stray one here and
/// there, as a share of the biome's `flowers`.
const STRAY_FLOWERS: f32 = 0.03;
/// Where the bloom field grows drifts of flowers: about a seventh of the land.
const DRIFTS: (f32, f32) = (0.82, 0.95);
/// The bloom field's rare peaks, where flowers grow densely, and how much denser.
const FLOWER_PATCHES: (f32, f32) = (0.985, 0.998);
const PATCH_BOOST: f32 = 4.0;
/// How many kinds of flower grow together in one place, about.
const KINDS_AT_ONCE: f32 = 2.5;
/// Margin over which a tree fades in and out of its band of the stand field.
const STAND_MARGIN: f32 = 0.08;
/// Chance per candidate cell of a weight-1 tree of a particular habitat (a willow by a
/// stream), where the habitat is just right.
const HABITAT_CHANCE: f32 = 0.15;
/// Chance per block of a weight-1 plant that grows alone.
const ALONE_CHANCE: f32 = 0.01;
/// Chance per block of a weight-1 wonder plant where aether is at its highest.
const WONDER_PLANT_CHANCE: f32 = 0.003;
/// Wonder trees outside the biomes that name them are a find: a lone tree or a tiny grove
/// of one kind, in at most one place per cell of this many blocks, ...
const WONDER_GROVE_CELL: i32 = 1024;
/// ... there with this chance where aether is high (fading out below), ...
const WONDER_GROVE_CHANCE: f32 = 0.6;
const WONDER_GROVE_AETHER: (f32, f32) = (0.55, 0.95);
/// ... over this radius in blocks (a lone tree to a handful), with this chance per
/// candidate cell inside.
const WONDER_GROVE_RADIUS: (f32, f32) = (2.5, 9.0);
const WONDER_IN_GROVE: f32 = 0.6;
const WONDER_GROVES: u64 = 0x6a07;
/// Below this share of its selected species suiting the climate, a biome's forest and
/// meadows thin out.
const SUITED: f32 = 0.3;
const PLANTS: u64 = 0x91a7;

/// A small plant: one block, a two-block plant, or a stack of one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plant {
    One(BlockId),
    Tall(BlockId, BlockId),
    Stack(BlockId, i32),
}

/// A plant's index in the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlantId(pub u16);

/// What generation needs of a tree definition.
struct TreeSpec {
    ground: u8,
    habitat: Habitat,
    comfort: Comfort,
    stands: bool,
    shades: bool,
    stand: Option<(f32, f32)>,
}

/// What generation needs of a plant definition.
struct PlantSpec {
    /// The plant's looks and their cumulative shares, ending at 1.
    looks: Vec<(BlockId, f32)>,
    top: Option<BlockId>,
    stack: Option<(i32, i32)>,
    ground: u8,
    habitat: Habitat,
    comfort: Comfort,
    snowy: Option<BlockId>,
}

/// What one biome grows: candidates from the pools with the biome's weight for each,
/// times their rarity and the world's featured boost.
#[derive(Default)]
struct Flora {
    forest: f32,
    cover: f32,
    flowers: f32,
    bloom: f32,
    /// Trees that make up the forest.
    canopy: Vec<(TreeId, f32)>,
    /// Trees of a particular habitat, beside the forest.
    habitat: Vec<(TreeId, f32)>,
    carpet: Vec<(PlantId, f32)>,
    drifts: Vec<(PlantId, f32)>,
    alone: Vec<(PlantId, f32)>,
    wonder_trees: Vec<(TreeId, f32)>,
    wonder_plants: Vec<(PlantId, f32)>,
}

/// What a plant is good for, for gameplay (cooking, alchemy, dyeing).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlantProperties {
    pub nutrition: f32,
    pub alchemy: Vec<String>,
    pub dye: Option<String>,
}

pub struct Ecology {
    trees: Vec<TreeSpec>,
    plants: Vec<PlantSpec>,
    plant_names: Vec<String>,
    /// Each plant's rarity times the world's featured boost.
    rarity: Vec<f32>,
    properties: Vec<PlantProperties>,
    /// Per biome, in biome order.
    flora: Vec<Flora>,
    /// The forest's structure: old-growth cores, woodland, scrubby edges and gaps.
    stands: FastNoiseLite,
    /// Clearings, across every biome.
    clearings: FastNoiseLite,
    /// Stands of one tree species.
    groves: FastNoiseLite,
    /// Lush and sparse grass.
    meadows: FastNoiseLite,
    /// Flower fields.
    blooms: FastNoiseLite,
    /// Which flower grows in a drift.
    kinds: FastNoiseLite,
    /// Which few kinds of flower grow in a region.
    varieties: FastNoiseLite,
    richness: f32,
    seed: u64,
}

fn ground_bit(ground: Ground) -> u8 {
    1 << ground as u8
}

/// The weight a biome's selectors give a candidate with this name and these tags: a
/// selector naming it wins, else the strongest matching tag; nothing if an excluded tag
/// matches.
pub(crate) fn selected<'a>(selects: &[Select], exclude: &[String], name: &str, tags: impl Iterator<Item = &'a str> + Clone) -> f32 {
    if tags.clone().any(|t| exclude.iter().any(|e| e == t)) {
        return 0.0;
    }
    let mut by_tag = 0.0f32;
    for select in selects {
        match select {
            Select::Name(n, w) if n == name => return *w,
            Select::Tag(tag, w) if tags.clone().any(|t| t == tag) => by_tag = by_tag.max(*w),
            _ => {}
        }
    }
    by_tag
}

/// How well a climate suits a comfort range: 1 inside, fading to 0 over a margin.
pub(crate) fn fit(comfort: &Comfort, temperature: f32, moisture: f32) -> f32 {
    let band = |v: f32, range: Option<(f32, f32)>, margin: f32| {
        range.map_or(1.0, |(lo, hi)| smoothstep(lo - margin, lo + margin, v) * (1.0 - smoothstep(hi - margin, hi + margin, v)))
    };
    band(temperature, comfort.temperature, 2.0) * band(moisture, comfort.moisture, 0.12)
}

/// How well a stand value suits a band of the stand field, 0 ..= 1: 1 inside, fading out
/// over a margin, except at the ends of the field.
fn stand_band(lo: f32, hi: f32, stand: f32) -> f32 {
    let rise = if lo <= 0.0 { 1.0 } else { smoothstep(lo - STAND_MARGIN, lo + STAND_MARGIN, stand) };
    let fall = if hi >= 1.0 { 1.0 } else { 1.0 - smoothstep(hi - STAND_MARGIN, hi + STAND_MARGIN, stand) };
    rise * fall
}

/// How much of its habitat a site offers, 0 ..= 1.
fn habitat(habitat: Habitat, site: &Site) -> f32 {
    // Height over the sea, lake or river over or beside the site.
    let above = site.height - site.water;
    let coast = site.continentalness < 0.74;
    let yes = |b: bool| if b { 1.0 } else { 0.0 };
    match habitat {
        Habitat::Anywhere => 1.0,
        Habitat::Open => 1.0 - 0.6 * site.forest,
        Habitat::Shade => smoothstep(0.3, 0.7, site.forest),
        Habitat::Understory => 0.25 + 0.75 * smoothstep(0.15, 0.6, site.forest),
        Habitat::Water => smoothstep(0.35, 0.7, site.river) * yes(above <= 3),
        Habitat::Shore => yes((0..=3).contains(&above) && (coast || site.river > 0.5)),
        Habitat::Shallows => yes((-2..=1).contains(&above) && (coast || site.river > 0.3)),
    }
}

impl Ecology {
    pub fn new(seed: u64, dials: &WorldDials, content: &WorldContent, registry: &BlockRegistry) -> Self {
        let rarities = |kind: &str, names: Vec<(&str, Rarity)>| {
            let boost = dials.featured.multipliers(kind, &names);
            names.iter().zip(boost).map(|((_, r), b)| r.multiplier() * b).collect::<Vec<f32>>()
        };
        let tree_rarity = rarities("tree", content.trees.iter().map(|t| (t.name.as_str(), t.rarity)).collect());
        let plant_rarity_of = |p: &PlantDef| p.rarity.unwrap_or_else(|| registry.def(registry.expect(&p.block)).rarity);
        let plant_rarity = rarities("plant", content.plants.iter().map(|p| (p.name.as_str(), plant_rarity_of(p))).collect());
        let mask = |grounds: &[Ground]| grounds.iter().fold(0, |m, g| m | ground_bit(*g));

        let trees = content
            .trees
            .iter()
            .map(|t: &TreeDef| TreeSpec {
                ground: mask(&t.ground),
                habitat: t.habitat,
                comfort: t.comfort.clone(),
                stands: t.stands,
                shades: t.crown.shades(),
                stand: t.stand,
            })
            .collect();
        let plants = content
            .plants
            .iter()
            .map(|p| {
                let looks = registry.looks(registry.expect(&p.block));
                let total: f32 = looks.iter().map(|l| l.rarity.multiplier()).sum();
                let mut sum = 0.0;
                PlantSpec {
                    looks: looks
                        .iter()
                        .map(|l| {
                            sum += l.rarity.multiplier() / total;
                            (l.block, sum)
                        })
                        .collect(),
                    top: p.top.as_deref().map(|n| registry.expect(n)),
                    stack: p.stack,
                    ground: mask(&p.ground),
                    habitat: p.habitat,
                    comfort: p.comfort.clone(),
                    snowy: p.snowy.as_deref().map(|n| registry.expect(n)),
                }
            })
            .collect();
        let properties = content.plants.iter().map(|p| plant_properties(content, p, registry)).collect();

        let flora = content.biomes.iter().map(|biome| Flora::new(&biome.flora, content, registry, &tree_rarity, &plant_rarity)).collect();

        Self {
            trees,
            plants,
            plant_names: content.plants.iter().map(|p| p.name.clone()).collect(),
            rarity: plant_rarity.clone(),
            properties,
            flora,
            stands: noise(seed, 30, 1.0 / 220.0, FractalType::FBm, 2),
            clearings: noise(seed, 35, 1.0 / 150.0, FractalType::FBm, 2),
            groves: noise(seed, 31, 1.0 / 140.0, FractalType::FBm, 2),
            meadows: noise(seed, 32, 1.0 / 40.0, FractalType::FBm, 2),
            blooms: noise(seed, 33, 1.0 / 90.0, FractalType::FBm, 2),
            kinds: noise(seed, 34, 1.0 / 22.0, FractalType::FBm, 2),
            varieties: noise(seed, 36, 1.0 / 600.0, FractalType::FBm, 2),
            richness: dials.vegetation,
            seed,
        }
    }

    /// Whether a tree casts a canopy (shrubs, snags and cacti do not).
    pub fn shades(&self, tree: TreeId) -> bool {
        self.trees[tree.0 as usize].shades
    }

    /// A plant's properties for gameplay, by name.
    pub fn plant_properties(&self, name: &str) -> Option<&PlantProperties> {
        self.plant_names.iter().position(|n| n == name).map(|i| &self.properties[i])
    }

    /// The forest's structure at a position, 0 ..= 1 and evenly spread: gaps and scrubby
    /// edges low, ordinary woodland in the middle, old-growth cores high.
    pub fn stand(&self, x: i32, z: i32) -> f32 {
        crate::climate::uniform(self.stands.get_noise_2d(x as f32, z as f32))
    }

    /// How deep into a clearing a position lies: 0 outside, rising across the soft edge to
    /// 1 where the clearing is open.
    pub fn clearing(&self, x: i32, z: i32) -> f32 {
        let u = crate::climate::uniform(self.clearings.get_noise_2d(x as f32, z as f32));
        smoothstep(CLEARING.0, CLEARING.1, u)
    }

    /// Tree cover (0..=1) at a position, given its stand and clearing: the forest of the
    /// biome there (and of the one next door, near a border), dense in old-growth cores,
    /// thin in gaps, gone in clearings, richer on fertile soil.
    pub fn forest(&self, stand: f32, clearing: f32, home: BiomeId, neighbour: BiomeId, share: f32, fertility: f32) -> f32 {
        let density = self.flora[home.0 as usize].forest * (1.0 - share) + self.flora[neighbour.0 as usize].forest * share;
        if density <= 0.0 {
            return 0.0;
        }
        let cover = 0.12 + 0.88 * smoothstep(0.08, 0.5, stand);
        let open = 1.0 - smoothstep(0.0, 0.45, clearing);
        (density * cover * open * (0.6 + 0.5 * self.richness) * (0.75 + 0.5 * fertility)).min(1.0)
    }

    /// The chance, per candidate cell, of each kind of tree growing here, into `out`. With
    /// `ground` unknown, ground does not restrict the kinds, which gives an upper bound.
    pub fn tree_chances(&self, site: &Site, ground: Option<Ground>, x: i32, z: i32, out: &mut Vec<(TreeId, f32)>) {
        out.clear();
        let flora = &self.flora[site.biome.0 as usize];
        let (t, m) = (site.temperature, site.soil_moisture());
        // Height over the sea, lake or river over or beside the site: trees need dry ground.
        let above = site.height - site.water;
        let suits = |tree: TreeId| {
            let spec = &self.trees[tree.0 as usize];
            let band = spec.stand.map_or(1.0, |(lo, hi)| stand_band(lo, hi, site.stand));
            fit(&spec.comfort, t, m) * habitat(spec.habitat, site) * band
        };

        // A wonder grove's tree comes first, so it takes the cells it grows in.
        if above >= 1
            && !flora.wonder_trees.is_empty()
            && let Some(u) = self.wonder_grove(site, x, z)
        {
            let total: f32 = flora.wonder_trees.iter().map(|&(tree, weight)| weight * suits(tree)).sum();
            let mut left = u * total;
            for &(tree, weight) in &flora.wonder_trees {
                let share = weight * suits(tree);
                if share > 0.0 && left < share {
                    out.push((tree, WONDER_IN_GROVE * suits(tree)));
                    break;
                }
                left -= share;
            }
        }
        if above >= 1 && site.forest > 0.0 && !flora.canopy.is_empty() {
            // Forest trees share the forest by how well each suits the climate, some in
            // stands of their own kind.
            let grove = smoothstep(-0.3, 0.3, self.groves.get_noise_2d(x as f32, z as f32));
            let mut total = 0.0;
            for &(tree, weight) in &flora.canopy {
                let stand = if self.trees[tree.0 as usize].stands { 0.2 + 1.6 * grove } else { 1.5 - grove };
                let share = weight * suits(tree) * stand;
                total += share;
                out.push((tree, share));
            }
            let all: f32 = flora.canopy.iter().map(|c| c.1).sum();
            let forest = site.forest * TREE_CHANCE * (total / (SUITED * all)).min(1.0);
            for entry in out.iter_mut() {
                entry.1 = if total > 0.0 { forest * entry.1 / total } else { 0.0 };
            }
        }
        // Trees of a particular habitat grow on their own terms.
        for &(tree, weight) in &flora.habitat {
            out.push((tree, HABITAT_CHANCE * weight * suits(tree)));
        }
        out.retain(|&(tree, chance)| chance > 0.0 && ground.is_none_or(|g| self.trees[tree.0 as usize].ground & ground_bit(g) != 0));
    }

    /// If (x, z) lies in a wonder grove, the roll that picks the grove's one kind of tree.
    /// Each cell of `WONDER_GROVE_CELL` blocks has at most one, at a place and of a size
    /// its hash decides, there more often the higher the aether.
    fn wonder_grove(&self, site: &Site, x: i32, z: i32) -> Option<f32> {
        let cell = WONDER_GROVE_CELL;
        let (gx, gz) = (x.div_euclid(cell), z.div_euclid(cell));
        let hash = hash3(self.seed ^ WONDER_GROVES, gx, 0, gz);
        let unit = |bits: u64| (bits & 0xffff) as f32 / 65536.0;
        let margin = 16.0;
        let span = cell as f32 - 2.0 * margin;
        let centre = (gx as f32 * cell as f32 + margin + span * unit(hash), gz as f32 * cell as f32 + margin + span * unit(hash >> 16));
        let radius = WONDER_GROVE_RADIUS.0 + (WONDER_GROVE_RADIUS.1 - WONDER_GROVE_RADIUS.0) * unit(hash >> 32).powi(2);
        let (dx, dz) = (x as f32 + 0.5 - centre.0, z as f32 + 0.5 - centre.1);
        if dx * dx + dz * dz > radius * radius {
            return None;
        }
        // Aether varies over thousands of blocks, so it barely changes across a grove.
        let gate = smoothstep(WONDER_GROVE_AETHER.0, WONDER_GROVE_AETHER.1, site.aether);
        (unit(hash >> 48) < WONDER_GROVE_CHANCE * gate).then(|| unit(hash3(self.seed ^ WONDER_GROVES, gx, 1, gz)))
    }

    /// The tree that dominates the canopy at a site, and the fraction of ground under
    /// crowns, for distant terrain. Shrubs, snags and cacti make no canopy.
    pub fn canopy(&self, site: &Site, ground: Ground, x: i32, z: i32) -> Option<(TreeId, f32)> {
        thread_local! {
            /// Reused between calls; it holds at most one entry per kind of tree.
            static CHANCES: std::cell::RefCell<Vec<(TreeId, f32)>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        CHANCES.with_borrow_mut(|chances| {
            self.tree_chances(site, Some(ground), x, z, chances);
            let crowns = chances.iter().filter(|(tree, _)| self.trees[tree.0 as usize].shades);
            let (total, dominant) = crowns.fold((0.0, None::<(TreeId, f32)>), |(total, best), &(tree, chance)| {
                (total + chance, if best.is_some_and(|(_, b)| b >= chance) { best } else { Some((tree, chance)) })
            });
            // A crown covers about six candidate cells' worth of ground; crowns overlap at random.
            dominant.map(|(tree, _)| (tree, 1.0 - (-1.5 * total).exp()))
        })
    }

    /// The small plant growing on `ground` at a site, if any. Each position decides alone,
    /// from a hash and smooth noise, so plants never depend on generation order. The
    /// stages take slices of one roll in a fixed order (flowers and wonders, then fungi and
    /// other plants that grow alone, then the carpet of grasses and ferns), so a stage can
    /// only take what the earlier ones left.
    pub fn plant(&self, site: &Site, ground: Ground, x: i32, z: i32) -> Option<Plant> {
        if matches!(ground, Ground::Bare | Ground::Shallows) {
            return None;
        }
        let flora = &self.flora[site.biome.0 as usize];
        let hash = hash3(self.seed ^ PLANTS, x, 0, z);
        let unit = |bits: u64| (bits & 0xff_ffff) as f32 / (1u32 << 24) as f32;
        let (second, third) = (unit(hash >> 24), unit(hash >> 40));
        let (t, m) = (site.temperature, site.soil_moisture());
        let snow = ground == Ground::Snow;
        let grows = |plant: PlantId| self.grows(plant, site, ground, t, m);
        // Each kind of growth takes a slice of one roll, as wide as its chance here.
        let mut roll = unit(hash);
        let mut take = |chance: f32| {
            let hit = roll < chance;
            roll -= chance;
            hit
        };
        // The weight of the candidates that grow here, and how much of the list's weight
        // they keep: below `SUITED`, the plants thin out.
        let suited = |list: &[(PlantId, f32)]| -> (f32, f32) {
            let (mut all, mut total) = (0.0, 0.0);
            for &(plant, weight) in list {
                all += weight;
                total += weight * grows(plant);
            }
            (total, if all > 0.0 { (total / (SUITED * all)).min(1.0) } else { 0.0 })
        };
        let choose = |list: &[(PlantId, f32)], total: f32, u: f32| choose(list, total, u, grows);
        let fertile = 0.8 + 0.4 * site.fertility;

        // Flowers, sparse: a stray one here and there, drifts where the bloom field runs
        // high, and dense patches only in its rare peaks (and throughout biomes in `bloom`,
        // such as flower meadows). A smooth noise chooses along the cumulative
        // weights, within a window a slow noise moves about, so neighbours mostly share a
        // kind and a place has only a few kinds at a time.
        if flora.flowers > 0.0 {
            let (total, thriving) = suited(&flora.drifts);
            if total > 0.0 {
                let bloom = crate::climate::uniform(self.blooms.get_noise_2d(x as f32, z as f32));
                let field = STRAY_FLOWERS + smoothstep(DRIFTS.0, DRIFTS.1, bloom).max(flora.bloom) + PATCH_BOOST * smoothstep(FLOWER_PATCHES.0, FLOWER_PATCHES.1, bloom);
                // Fewest deep in the woods; more at forest edges, in clearings and along
                // creeks and rivers.
                let edge = smoothstep(0.05, 0.25, site.forest) * (1.0 - smoothstep(0.35, 0.6, site.forest));
                let open = (1.0 - 0.75 * site.forest) * (1.0 + 0.5 * site.clearing + 1.2 * edge + 1.5 * site.river);
                if take(flora.flowers * field * open * thriving * (0.7 + 0.6 * self.richness) * fertile) {
                    let variety = crate::climate::uniform(self.varieties.get_noise_2d(x as f32, z as f32));
                    let local = crate::climate::uniform(self.kinds.get_noise_2d(x as f32, z as f32)) + 0.1 * (second - 0.5);
                    let window = (KINDS_AT_ONCE / flora.drifts.len() as f32).min(1.0);
                    let drift = (variety + window * local.clamp(0.0, 1.0)).rem_euclid(1.0);
                    return Some(self.place(choose(&flora.drifts, total, drift), third, snow));
                }
            }
        }
        // Rare fantastical plants, more common where aether is high.
        let aether = site.aether * site.aether;
        for &(plant, weight) in &flora.wonder_plants {
            if take(WONDER_PLANT_CHANCE * weight * grows(plant) * aether) {
                return Some(self.place(plant, second, snow));
            }
        }
        // Fungi, bushes, reeds: plants that grow alone.
        for &(plant, weight) in &flora.alone {
            if take(ALONE_CHANCE * weight * grows(plant)) {
                return Some(self.place(plant, second, snow));
            }
        }
        // Carpets of grass and ferns over most of what is left, lusher in patches and in
        // clearings.
        if flora.cover > 0.0 {
            let (total, thriving) = suited(&flora.carpet);
            if total > 0.0 {
                let meadow = 0.7 + 0.3 * smoothstep(-0.5, 0.4, self.meadows.get_noise_2d(x as f32, z as f32));
                let lush = (meadow + 0.3 * site.clearing).min(1.0);
                if take(flora.cover * site.vegetation * lush * thriving * fertile) {
                    return Some(self.place(choose(&flora.carpet, total, second), third, snow));
                }
            }
        }
        None
    }

    /// How well a plant grows on `ground` at a site with temperature `t` and soil moisture
    /// `m`, 0 ..= 1. Under snow, only plants with a snowy look grow, where they would on soil.
    fn grows(&self, plant: PlantId, site: &Site, ground: Ground, t: f32, m: f32) -> f32 {
        let spec = &self.plants[plant.0 as usize];
        let rooted = spec.ground & ground_bit(ground) != 0 || (ground == Ground::Snow && spec.snowy.is_some() && spec.ground & ground_bit(Ground::Soil) != 0);
        if !rooted {
            return 0.0;
        }
        fit(&spec.comfort, t, m) * habitat(spec.habitat, site)
    }

    /// A plant from `list` (weights) that suits a site, with `u` and `look` in 0..1, or
    /// none if none of them grows there. For decorations: fungi on logs, plants at the
    /// foot of trunks.
    pub fn pick(&self, list: &[(PlantId, f32)], site: &Site, ground: Ground, u: f32, look: f32) -> Option<Plant> {
        let (t, m) = (site.temperature, site.soil_moisture());
        let grows = |plant: PlantId| self.grows(plant, site, ground, t, m);
        let total: f32 = list.iter().map(|&(p, w)| w * grows(p)).sum();
        (total > 0.0).then(|| self.place(choose(list, total, u, grows), look, ground == Ground::Snow))
    }

    /// Plants a decoration names by selector, with their weights (times rarity).
    pub fn select(&self, selects: &[Select], content: &WorldContent, registry: &BlockRegistry) -> Vec<(PlantId, f32)> {
        content
            .plants
            .iter()
            .enumerate()
            .filter_map(|(i, plant)| {
                let weight = selected(selects, &[], &plant.name, content.plant_tags(plant, registry)) * self.rarity[i];
                (weight > 0.0).then_some((PlantId(i as u16), weight))
            })
            .collect()
    }

    /// A plant's blocks, with its look (and a stack's height) chosen by `u` in 0..1; its
    /// snowy look where it grows under snow.
    fn place(&self, plant: PlantId, u: f32, snow: bool) -> Plant {
        let spec = &self.plants[plant.0 as usize];
        if let Some(snowy) = spec.snowy.filter(|_| snow) {
            return Plant::One(snowy);
        }
        let look = spec.looks.iter().find(|l| u < l.1).unwrap_or(&spec.looks[spec.looks.len() - 1]).0;
        match (spec.top, spec.stack) {
            (Some(top), _) => Plant::Tall(look, top),
            (None, Some((lo, hi))) => Plant::Stack(look, lo + ((u * 7919.0) as i32).rem_euclid(hi - lo + 1)),
            (None, None) => Plant::One(look),
        }
    }
}

/// Picks along the cumulative weights of the plants in `list` that grow here (`grows`),
/// whose weights add up to `total`, with `u` in 0..1.
fn choose(list: &[(PlantId, f32)], total: f32, u: f32, grows: impl Fn(PlantId) -> f32) -> PlantId {
    let mut left = u * total;
    let mut last = list[0].0;
    for &(plant, weight) in list {
        let weight = weight * grows(plant);
        if weight > 0.0 {
            if left < weight {
                return plant;
            }
            left -= weight;
            last = plant;
        }
    }
    last
}

impl Flora {
    /// Resolves a biome's selectors against the pools, once per world.
    fn new(def: &FloraDef, content: &WorldContent, registry: &BlockRegistry, tree_rarity: &[f32], plant_rarity: &[f32]) -> Self {
        let wonders = def.wonders.clone().unwrap_or_else(|| vec![Select::Tag("wonder".into(), 1.0)]);
        let mut flora = Flora { forest: def.forest, cover: def.cover, flowers: def.flowers, bloom: def.bloom, ..Default::default() };
        for (i, tree) in content.trees.iter().enumerate() {
            let tags = tree.tags.iter().map(String::as_str);
            let id = TreeId(i as u16);
            // A tree the biome names is part of its forest, wonder or not.
            let named = def.trees.iter().any(|s| matches!(s, Select::Name(n, _) if *n == tree.name));
            let wonder = if named { 0.0 } else { selected(&wonders, &def.exclude, &tree.name, tags.clone()) * tree_rarity[i] };
            if wonder > 0.0 {
                flora.wonder_trees.push((id, wonder));
                continue;
            }
            let weight = selected(&def.trees, &def.exclude, &tree.name, tags) * tree_rarity[i];
            if weight > 0.0 {
                let own_terms = matches!(tree.habitat, Habitat::Water | Habitat::Shore | Habitat::Shallows);
                if own_terms { &mut flora.habitat } else { &mut flora.canopy }.push((id, weight));
            }
        }
        for (i, plant) in content.plants.iter().enumerate() {
            let tags = content.plant_tags(plant, registry);
            let id = PlantId(i as u16);
            let wonder = selected(&wonders, &def.exclude, &plant.name, tags.clone()) * plant_rarity[i];
            if wonder > 0.0 {
                flora.wonder_plants.push((id, wonder));
                continue;
            }
            let weight = selected(&def.plants, &def.exclude, &plant.name, tags) * plant_rarity[i];
            if weight > 0.0 {
                match plant.spread {
                    Spread::Cover => flora.carpet.push((id, weight)),
                    Spread::Drifts => flora.drifts.push((id, weight)),
                    Spread::Alone => flora.alone.push((id, weight)),
                }
            }
        }
        flora
    }
}

/// A plant's properties: those it sets, else what its block's tags say (`edible`,
/// `alchemy:<property>`, `dye:<colour>`).
fn plant_properties(content: &WorldContent, plant: &PlantDef, registry: &BlockRegistry) -> PlantProperties {
    let tags: Vec<&str> = content.plant_tags(plant, registry).collect();
    let given = &plant.properties;
    PlantProperties {
        nutrition: given.nutrition.unwrap_or(if tags.contains(&"edible") { 1.0 } else { 0.0 }),
        alchemy: given
            .alchemy
            .clone()
            .unwrap_or_else(|| tags.iter().filter_map(|t| t.strip_prefix("alchemy:")).map(str::to_owned).collect()),
        dye: given.dye.clone().or_else(|| tags.iter().find_map(|t| t.strip_prefix("dye:")).map(str::to_owned)),
    }
}

/// Ground cover (0..=1) the climate supports: grass needs less water than trees and
/// tolerates more cold.
pub fn ground_cover(temperature: f32, water: f32) -> f32 {
    smoothstep(0.1, 0.55, water) * smoothstep(-6.0, 4.0, temperature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::BASE_FILES;
    use voxmar_engine::content::ContentFile;

    fn registry() -> BlockRegistry {
        BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap()
    }

    fn content(extra: &[(&str, &str)], registry: &BlockRegistry) -> WorldContent {
        let files: Vec<ContentFile> = BASE_FILES
            .iter()
            .map(|(name, source)| ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() })
            .chain(extra.iter().map(|(name, source)| ContentFile { package: "mod".into(), name: (*name).into(), source: (*source).into() }))
            .collect();
        WorldContent::from_files(files.iter(), registry).unwrap()
    }

    #[test]
    fn a_new_plant_grows_wherever_its_tags_fit() {
        let registry = registry();
        // A mod's flower, tagged for meadows, touching no biome.
        let content = content(&[("plants.ron", "(plants: [(name: \"star_poppy\", block: \"base:poppy\", tags: [\"meadow\"], spread: Drifts)])")], &registry);
        let ecology = Ecology::new(1, &WorldDials::sample(1), &content, &registry);
        let flowers_of = |biome: &str| {
            let i = content.biomes.iter().position(|b| b.name == biome).unwrap();
            ecology.flora[i].drifts.iter().map(|(p, _)| ecology.plant_names[p.0 as usize].as_str()).collect::<Vec<_>>()
        };
        assert!(flowers_of("base:plains").contains(&"mod:star_poppy"));
        assert!(flowers_of("base:flower_meadow").contains(&"mod:star_poppy"));
        assert!(!flowers_of("base:desert").contains(&"mod:star_poppy"));
    }

    #[test]
    fn climate_decides_between_candidates() {
        let comfort = Comfort { temperature: Some((5.0, 15.0)), moisture: None };
        assert_eq!(fit(&comfort, 10.0, 1.0), 1.0);
        assert_eq!(fit(&comfort, 25.0, 1.0), 0.0);
        assert!((0.0..1.0).contains(&fit(&comfort, 16.0, 1.0)));
        // A name outweighs a tag; an excluded tag removes the candidate.
        let selects = [Select::Tag("broadleaf".into(), 1.0), Select::Name("base:oak".into(), 3.0)];
        assert_eq!(selected(&selects, &[], "base:oak", ["broadleaf"].into_iter()), 3.0);
        assert_eq!(selected(&selects, &[], "base:birch", ["broadleaf"].into_iter()), 1.0);
        assert_eq!(selected(&selects, &["broadleaf".into()], "base:oak", ["broadleaf"].into_iter()), 0.0);
    }

    #[test]
    fn plants_carry_properties_from_their_tags() {
        let registry = registry();
        let content = content(&[], &registry);
        let ecology = Ecology::new(1, &WorldDials::sample(1), &content, &registry);
        let dandelion = ecology.plant_properties("base:dandelion").unwrap();
        assert_eq!((dandelion.dye.as_deref(), dandelion.alchemy.clone(), dandelion.nutrition), (Some("yellow"), vec!["healing".to_owned()], 1.0));
        assert_eq!(ecology.plant_properties("base:red_mushroom").unwrap().alchemy, vec!["poison".to_owned()]);
    }
}
