//! Stage 3: what grows where. Trees and plants are a shared pool defined as data
//! (`trees.ron`, `plants.ron`); each biome selects from the pool by tag or name, and at
//! every spot a candidate's weight is the biome's weight for it times how well the
//! climate there suits it, times its rarity. So the same pool varies smoothly inside a
//! biome and across biomes: oaks give way to birches as it gets cooler, and a mod's new
//! flower grows wherever its tags and climate fit.
//!
//! A biome keeps to a palette (`crate::hue`): candidates are weighed by how well their
//! colour sits with its canopy's anchor colour (its foliage, or a colour set in data) and
//! the carpet's (its grass), so many kinds grow together where their colours agree and a
//! clashing one is a rare accent; flowers are ordered by hue, so the few kinds growing in
//! one place are neighbours in colour.
//!
//! Low-frequency noise adds the patchiness of real vegetation. One field, the stand, grades
//! a forest's structure from old-growth cores (dense, tall, with the giants) through
//! ordinary woodland to scrubby edges and gaps (Terralith's band-pass forests), so tree
//! density and tree size change together. A second, global field opens clearings with a
//! soft edge across every biome: trees fade out first, shrubs reach further in, and meadow
//! plants and the clearing's own decorations fill them (Wilder Wild's clearings). Further
//! noise makes stands of one tree species with a few others among them, lusher and sparser
//! meadows, and drifts of one kind of flower. Where aether runs high, rare fantastical
//! trees and plants turn up.
//!
//! Small plants are decided block by block in a fixed order of stages: flowers (and
//! wonders), fungi and other plants that grow alone, then the grasses and ferns that
//! carpet whatever is left, so most open ground in a meadow carries a plant.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::biome::BiomeId;
use crate::content::{BiomeDef, Comfort, Habitat, PlantDef, Select, Spread, TreeDef, WorldContent};
use crate::hue;
use crate::dials::WorldDials;
use crate::geology::{noise, smoothstep};
use crate::levels::capped;
use crate::rarity::{Rarity, Tier};
use crate::rng::hash3;
use crate::trees::TreeId;
use crate::Site;
use voxmar_engine::glam::IVec2;

pub use crate::content::Ground;

/// Chance that a tree grows on a 4×4 candidate cell where the forest is densest.
pub const TREE_CHANCE: f32 = 0.6;
/// The exponent a biome's `cover` is eased by: a meadow's 0.95 stays about 0.92, a
/// forest's 0.75 becomes about 0.63.
const COVER_EASE: f32 = 1.6;
/// The least share of a biome's forest a kind needs to lead stands of its own.
const STAND_LEADERS: f32 = 0.15;
/// The share of a stand's trees of its own kind; the others grow among them.
const STAND_PURITY: f32 = 0.9;
/// Where the clearing field (evenly spread over 0 ..= 1) starts to open a clearing, and
/// where the clearing is fully open: about a tenth of the land lies in clearings.
const CLEARING: (f32, f32) = (0.85, 0.95);
/// Flowers where the bloom field (evenly spread over 0 ..= 1) is low: a stray one here and
/// there, as a share of the biome's `flowers`.
const STRAY_FLOWERS: f32 = 0.015;
/// Where the bloom field grows drifts of flowers: about a fifteenth of the land.
const DRIFTS: (f32, f32) = (0.9, 0.98);
/// The bloom field's rare peaks, where flowers grow densely, and how much denser.
const FLOWER_PATCHES: (f32, f32) = (0.985, 0.998);
const PATCH_BOOST: f32 = 4.0;
/// How many kinds of flower grow together in one place, about: neighbours in hue.
const KINDS_AT_ONCE: f32 = 2.0;
/// Margin over which a tree fades in and out of its band of the stand field.
const STAND_MARGIN: f32 = 0.08;
/// Chance per candidate cell of a weight-1 tree of a particular habitat (a willow by a
/// stream), where the habitat is just right.
const HABITAT_CHANCE: f32 = 0.15;
/// Chance per block of a weight-1 plant that grows alone.
const ALONE_CHANCE: f32 = 0.01;
/// Chance per block of a weight-1 wonder plant where aether is at its highest.
const WONDER_PLANT_CHANCE: f32 = 0.003;
/// Rare herbs grow in small patches (`PlantDef::patches`): the land is cut into cells this
/// many blocks across, and each cell holds at most one patch of each herb, its plants
/// within two blocks of its centre and its centre this far inside the cell.
pub(crate) const PATCH_CELL: i32 = 32;
const PATCH_MARGIN: i32 = 3;
const PATCHES: u64 = 0x3b5d;
/// Patches of a rare herb along water (`Ecology::edge_patches`) start at least this many
/// blocks apart, and take the columns of their habitat within this reach of their start.
const EDGE_SPACING: i32 = 7;
const EDGE_REACH: i32 = 3;
const EDGES: u64 = 0x5e9a;
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
const STANDS: u64 = 0x57a4;

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
    /// A rare herb's patches: plants per million blocks, and the least and most in a patch.
    patches: Option<(f32, u32, u32)>,
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
    /// Rare herbs the biome names that grow in patches, with the weight it names each with.
    patch_herbs: Vec<(PlantId, f32)>,
    /// Those of them whose habitat runs along the edge of water (`Habitat::edge`), whose
    /// patches start from it (`Ecology::edge_patches`).
    edge_herbs: Vec<(PlantId, f32)>,
    /// The biome's name hashed, which keys its edge herbs' patches: the same whatever order
    /// the biomes load in.
    name: u64,
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
    /// The cold conifers that take over a forest where none of its biome's own trees can
    /// grow, as forests give way to conifers towards a treeline.
    hardy: Vec<(TreeId, f32)>,
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
    /// The content's tree density, on forest trees.
    tree_density: f32,
    /// Whether any biome grows a rare herb along water (`edge_patches`).
    edges: bool,
    /// Each plant's share of its biome's rate that grows: the content's plant density,
    /// or all of it for the plants the density keeps (gatherables).
    plant_density: Vec<f32>,
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
        Habitat::LakeShore => yes(site.body == crate::Body::Lake && (0..=2).contains(&above)),
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
                    patches: p.patches.as_ref().map(|q| (q.rate, q.size.0, q.size.1)),
                }
            })
            .collect();
        let properties = content.plants.iter().map(|p| plant_properties(content, p, registry)).collect();

        // Each biome's level makes its rarer plants and gathering trees commoner
        // (`crate::levels`); other trees, the canopy's own kinds, keep their shares (no class
        // the levels scale).
        let scaled = |t: &TreeDef| t.tags.iter().any(|tag| content.climate.levels.trees.contains(tag));
        let tree_class: Vec<Option<Rarity>> = content.trees.iter().map(|t| scaled(t).then_some(t.rarity)).collect();
        let plant_class: Vec<Option<Rarity>> = content.plants.iter().map(|p| Some(plant_rarity_of(p))).collect();
        let flora: Vec<Flora> = content
            .biomes
            .iter()
            .map(|biome| {
                let levels = &content.climate.levels;
                let at_level = |classes: &[Option<Rarity>], rarity: &[f32]| -> Vec<f32> {
                    classes.iter().zip(rarity).map(|(class, &m)| class.map_or(m, |class| capped(class, m * levels.finds(biome.level, class)))).collect()
                };
                Flora::new(biome, content, registry, &at_level(&tree_class, &tree_rarity), &at_level(&plant_class, &plant_rarity), &plant_rarity)
            })
            .collect();
        let edges = flora.iter().any(|f| !f.edge_herbs.is_empty());

        let hardy = content
            .trees
            .iter()
            .enumerate()
            .filter(|(_, t)| t.tags.iter().any(|tag| tag == "conifer") && t.tags.iter().any(|tag| tag == "cold") && t.rarity == Rarity::Common)
            .map(|(i, _)| (TreeId(i as u16), 1.0))
            .collect();
        Self {
            hardy,
            trees,
            plants,
            plant_names: content.plants.iter().map(|p| p.name.clone()).collect(),
            rarity: plant_rarity.clone(),
            properties,
            flora,
            stands: noise(seed, 30, 1.0 / 220.0, FractalType::FBm, 2),
            clearings: noise(seed, 35, 1.0 / 150.0, FractalType::FBm, 2),
            groves: noise(seed, 31, 1.0 / 160.0, FractalType::FBm, 2),
            meadows: noise(seed, 32, 1.0 / 40.0, FractalType::FBm, 2),
            blooms: noise(seed, 33, 1.0 / 90.0, FractalType::FBm, 2),
            kinds: noise(seed, 34, 1.0 / 22.0, FractalType::FBm, 2),
            varieties: noise(seed, 36, 1.0 / 600.0, FractalType::FBm, 2),
            richness: dials.vegetation,
            tree_density: content.tree_density,
            edges,
            plant_density: content.plants.iter().map(|p| if content.plant_kept(p, registry) { 1.0 } else { content.plant_density }).collect(),
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

    /// Tree cover (0..=1) at a position, given its stand and clearing and its climate
    /// (°C, soil moisture): the forest of the biome there (and of the one next door, near
    /// a border), dense in old-growth cores, thin in gaps, gone in clearings, richer on
    /// fertile soil, and thinning where the climate suits few of its trees (towards the
    /// tree line), so everything that follows tree cover (forest floors, litter, flowers
    /// at the edge) follows the trees that actually stand.
    #[allow(clippy::too_many_arguments)]
    pub fn forest(&self, stand: f32, clearing: f32, home: BiomeId, neighbour: BiomeId, share: f32, fertility: f32, t: f32, m: f32) -> f32 {
        let suited = |biome: BiomeId| {
            let flora = &self.flora[biome.0 as usize];
            let all: f32 = flora.canopy.iter().map(|c| c.1).sum();
            let total: f32 = flora.canopy.iter().map(|&(tree, w)| w * fit(&self.trees[tree.0 as usize].comfort, t, m)).sum();
            if all > 0.0 { (total / (SUITED * all)).min(1.0) } else { 1.0 }
        };
        let density = self.flora[home.0 as usize].forest * suited(home) * (1.0 - share) + self.flora[neighbour.0 as usize].forest * suited(neighbour) * share;
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
        // The biome whose land this is rather than the dithered pick near a border, so
        // stands of different colours meet along the border instead of mixing across it;
        // the banks of rivers and lakes keep their own trees.
        let biome = if site.biome == site.neighbour { site.home } else { site.biome };
        let flora = &self.flora[biome.0 as usize];
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
            // Up a hill too cold for every tree the biome lists (biomes follow the regional
            // climate, the trees the climate of the spot), the forest gives way to the hardy
            // conifers, as forests do towards a treeline.
            let canopy = if flora.canopy.iter().any(|&(tree, _)| suits(tree) > 0.0) { &flora.canopy } else { &self.hardy };
            // The forest grows in stands: a slow noise picks each stand's kind along the
            // weights of the kinds suited here, and the others grow among it, a few.
            let start = out.len();
            let mut total = 0.0;
            for &(tree, weight) in canopy {
                let share = weight * suits(tree);
                total += share;
                out.push((tree, share));
            }
            let all: f32 = canopy.iter().map(|c| c.1).sum();
            let forest = site.forest * TREE_CHANCE * self.tree_density * (total / (SUITED * all)).min(1.0);
            let jitter = (hash3(self.seed ^ STANDS, x, 0, z) & 0xffff) as f32 / 65536.0 - 0.5;
            let u = (crate::climate::uniform(self.groves.get_noise_2d(x as f32, z as f32)) + 0.04 * jitter).clamp(0.0, 0.9999);
            // Only the main kinds (each at least `STAND_LEADERS` of the forest) lead a stand;
            // the rest are accents that grow only among the others' stands.
            let canopy = &mut out[start..];
            let leading = |share: f32| share >= STAND_LEADERS * total;
            let leaders: f32 = canopy.iter().map(|c| c.1).filter(|&s| leading(s)).sum();
            let mut left = u * leaders;
            let dominant = canopy.iter().position(|&(_, share)| {
                if !leading(share) {
                    return false;
                }
                let hit = share > 0.0 && left < share;
                left -= share;
                hit
            });
            let others = dominant.map_or(0.0, |d| total - canopy[d].1);
            for (i, entry) in canopy.iter_mut().enumerate() {
                entry.1 = match dominant {
                    Some(d) if i == d => forest * if others > 0.0 { STAND_PURITY } else { 1.0 },
                    Some(_) if others > 0.0 => forest * (1.0 - STAND_PURITY) * entry.1 / others,
                    _ => 0.0,
                };
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
        if ground == Ground::Shallows {
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
        // Each plant of a list's `grows` here, worked out once for the list (`suited`), not
        // again for its share (`kept`) and its choice (`choose_thinned`).
        let mut grown: Vec<f32> = Vec::new();
        let suited = |grown: &mut Vec<f32>, list: &[(PlantId, f32)]| -> (f32, f32) {
            grown.clear();
            grown.extend(list.iter().map(|&(plant, _)| grows(plant)));
            let (mut all, mut total) = (0.0, 0.0);
            for (&(_, weight), &g) in list.iter().zip(grown.iter()) {
                all += weight;
                total += weight * g;
            }
            (total, if all > 0.0 { (total / (SUITED * all)).min(1.0) } else { 0.0 })
        };
        // As `grows`, thinned by the plant density (gatherables keep their rate), and the
        // share of a list's weight here that survives the thinning.
        let thinned = |plant: PlantId| grows(plant) * self.plant_density[plant.0 as usize];
        let thinned_in = |grown: &[f32], list: &[(PlantId, f32)], i: usize| grown[i] * self.plant_density[list[i].0.0 as usize];
        let kept = |grown: &[f32], list: &[(PlantId, f32)], total: f32| (0..list.len()).map(|i| list[i].1 * thinned_in(grown, list, i)).sum::<f32>() / total;
        let choose_thinned = |grown: &[f32], list: &[(PlantId, f32)], total: f32, u: f32| {
            choose_with(list, total * kept(grown, list, total), u, |i| thinned_in(grown, list, i))
        };
        let fertile = 0.8 + 0.4 * site.fertility;

        // Rare herbs, in their patches, in the biome the position belongs to (banks and
        // borders included), wherever the ground and habitat suit each plant: the biomes
        // that name a herb already choose its climate, so its comfort does not thin it.
        for &(plant, weight) in &self.flora[site.home.0 as usize].patch_herbs {
            if self.in_patch(plant, weight, x, z) && third < self.rooted(plant, ground) * habitat(self.plants[plant.0 as usize].habitat, site) {
                return Some(self.place(plant, second, snow));
            }
        }
        // Nothing else grows on bare rock, gravel or ice.
        if ground == Ground::Bare {
            return None;
        }
        // Flowers, sparse: a stray one here and there, drifts where the bloom field runs
        // high, and dense patches only in its rare peaks (and throughout biomes in `bloom`,
        // such as flower meadows). A smooth noise chooses along the cumulative
        // weights, within a window a slow noise moves about, so neighbours mostly share a
        // kind and a place has only a few kinds at a time.
        if flora.flowers > 0.0 {
            let (total, thriving) = suited(&mut grown, &flora.drifts);
            if total > 0.0 {
                let bloom = crate::climate::uniform(self.blooms.get_noise_2d(x as f32, z as f32));
                let field = STRAY_FLOWERS + smoothstep(DRIFTS.0, DRIFTS.1, bloom).max(flora.bloom) + PATCH_BOOST * smoothstep(FLOWER_PATCHES.0, FLOWER_PATCHES.1, bloom);
                // Fewest deep in the woods; more at forest edges, in clearings and along
                // creeks and rivers.
                let edge = smoothstep(0.05, 0.25, site.forest) * (1.0 - smoothstep(0.35, 0.6, site.forest));
                let open = (1.0 - 0.75 * site.forest) * (1.0 + 0.5 * site.clearing + 1.2 * edge + 1.5 * site.river);
                if take(kept(&grown, &flora.drifts, total) * flora.flowers * field * open * thriving * (0.7 + 0.6 * self.richness) * fertile) {
                    let variety = crate::climate::uniform(self.varieties.get_noise_2d(x as f32, z as f32));
                    let local = crate::climate::uniform(self.kinds.get_noise_2d(x as f32, z as f32)) + 0.1 * (second - 0.5);
                    let window = (KINDS_AT_ONCE / flora.drifts.len() as f32).min(1.0);
                    let drift = (variety + window * local.clamp(0.0, 1.0)).rem_euclid(1.0);
                    return Some(self.place(choose_thinned(&grown, &flora.drifts, total, drift), third, snow));
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
            if take(ALONE_CHANCE * weight * thinned(plant)) {
                return Some(self.place(plant, second, snow));
            }
        }
        // Carpets of grass and ferns over most of what is left, lusher in patches and in
        // clearings.
        if flora.cover > 0.0 {
            let (total, thriving) = suited(&mut grown, &flora.carpet);
            if total > 0.0 {
                let meadow = 0.7 + 0.3 * smoothstep(-0.5, 0.4, self.meadows.get_noise_2d(x as f32, z as f32));
                let lush = (meadow + 0.3 * site.clearing).min(1.0);
                // The biome's cover, eased so the lushest meadows keep theirs while
                // ordinary ground is a little thinner.
                if take(kept(&grown, &flora.carpet, total) * flora.cover.powf(COVER_EASE) * site.vegetation * lush * thriving * fertile) {
                    return Some(self.place(choose_thinned(&grown, &flora.carpet, total, second), third, snow));
                }
            }
        }
        None
    }

    /// The plants of the patches of rare herbs whose habitat runs along water
    /// (`Habitat::edge`: a lake's bank, a beach, a river's edge) in the cell of `PATCH_CELL`
    /// blocks whose first column is `first`, as each one's column (its index in the cell,
    /// row by row) and plant. A patch at a random spot, as other rare herbs have
    /// (`in_patch`), would seldom touch so thin a line, so these start from it: each such
    /// herb takes patches in each biome of the cell naming it, in proportion to that biome's
    /// columns of its habitat there (its rate counts plants per million of them, times the
    /// biome's weight), each started at one of those columns chosen by hash, at least
    /// `EDGE_SPACING` blocks from the herb's other patches in the cell (of any biome, so two
    /// at a border never run together), of as many of the nearest such columns of the
    /// same biome within `EDGE_REACH` as its size. `column(i)` gives the site and ground of the cell's i-th
    /// column, or none where nothing may grow (under water, under a tree, on a trail). Only
    /// the cell's own columns decide, so a cell is the same whatever generated it.
    pub fn edge_patches<'s>(&self, first: IVec2, column: impl Fn(usize) -> Option<(&'s Site, Ground)>) -> Vec<(usize, Plant)> {
        let mut out = Vec::new();
        if !self.edges {
            return out;
        }
        let side = PATCH_CELL as usize;
        // The columns of each herb's habitat, by herb and biome, with the weight their biome
        // names it with: each biome's columns make and hold its own patches, so a biome
        // naming the herb heavily does not grow it on a lighter neighbour's bank.
        let mut found: Vec<((PlantId, BiomeId), Vec<(usize, f32)>)> = Vec::new();
        for i in 0..side * side {
            let Some((site, ground)) = column(i) else { continue };
            for &(plant, weight) in &self.flora[site.home.0 as usize].edge_herbs {
                if self.rooted(plant, ground) == 0.0 || habitat(self.plants[plant.0 as usize].habitat, site) < 0.5 {
                    continue;
                }
                match found.iter_mut().find(|(p, _)| *p == (plant, site.home)) {
                    Some((_, columns)) => columns.push((i, weight)),
                    None => found.push(((plant, site.home), vec![(i, weight)])),
                }
            }
        }
        let cell = first.div_euclid(IVec2::splat(PATCH_CELL));
        let unit = |bits: u64| (bits & 0xff_ffff) as f32 / (1u32 << 24) as f32;
        let at = |i: usize| IVec2::new(first.x + (i % side) as i32, first.y + (i / side) as i32);
        // Each herb's patch starts in the cell, whichever biome's they are.
        let mut started: Vec<(PlantId, Vec<IVec2>)> = Vec::new();
        for ((plant, home), columns) in found {
            let Some((rate, least, most)) = self.plants[plant.0 as usize].patches else { continue };
            let seed = self.seed ^ EDGES ^ (plant.0 as u64) << 24 ^ self.flora[home.0 as usize].name;
            // As many patches as the habitat's columns make on average, rounded up or down
            // by a roll.
            let mean = (least + most) as f32 / 2.0;
            let expected: f32 = columns.iter().map(|&(_, weight)| rate * weight / 1e6).sum::<f32>() / mean;
            let count = (expected + unit(hash3(seed, cell.x, 0, cell.y))) as usize;
            if count == 0 {
                continue;
            }
            let mut order: Vec<(u64, usize)> = columns.iter().map(|&(i, _)| (hash3(seed, at(i).x, 1, at(i).y), i)).collect();
            order.sort_unstable();
            let starts = match started.iter().position(|(p, _)| *p == plant) {
                Some(k) => &mut started[k].1,
                None => {
                    started.push((plant, Vec::new()));
                    &mut started.last_mut().expect("just pushed").1
                }
            };
            let mut taken: Vec<usize> = Vec::new();
            let mut made = 0;
            for &(hash, i) in &order {
                if made >= count {
                    break;
                }
                let start = at(i);
                if starts.iter().any(|s| (*s - start).abs().max_element() < EDGE_SPACING) {
                    continue;
                }
                starts.push(start);
                made += 1;
                let size = least as usize + (hash >> 40) as usize % (most - least + 1) as usize;
                let mut near: Vec<(i32, u64, usize)> = order
                    .iter()
                    .filter(|&&(_, j)| (at(j) - start).abs().max_element() <= EDGE_REACH && !taken.contains(&j))
                    .map(|&(h, j)| ((at(j) - start).length_squared(), h, j))
                    .collect();
                near.sort_unstable();
                taken.extend(near.iter().take(size).map(|&(_, _, j)| j));
            }
            for i in taken {
                let p = at(i);
                let look = unit(hash3(self.seed ^ PLANTS, p.x, 0, p.y) >> 24);
                let snow = column(i).is_some_and(|(_, ground)| ground == Ground::Snow);
                out.push((i, self.place(plant, look, snow)));
            }
        }
        out
    }

    /// Whether (x, z) holds a plant of a rare herb's patch, for a biome naming it with
    /// `weight`. Each cell of `PATCH_CELL` blocks holds a patch with the chance that makes
    /// the herb's rate, centred where its hash says, of a size from its range: the centre and
    /// the rest at hashed spots within two blocks of it (two may fall together).
    fn in_patch(&self, plant: PlantId, weight: f32, x: i32, z: i32) -> bool {
        let Some((rate, least, most)) = self.plants[plant.0 as usize].patches else { return false };
        let (gx, gz) = (x.div_euclid(PATCH_CELL), z.div_euclid(PATCH_CELL));
        let seed = self.seed ^ PATCHES ^ (plant.0 as u64) << 24;
        let hash = hash3(seed, gx, 0, gz);
        let unit = |bits: u64| (bits & 0xffff) as f32 / 65536.0;
        let mean = (least + most) as f32 / 2.0;
        let chance = rate * weight * (PATCH_CELL * PATCH_CELL) as f32 / (mean * 1e6);
        if unit(hash) >= chance {
            return false;
        }
        let span = (PATCH_CELL - 2 * PATCH_MARGIN) as f32;
        let cx = gx * PATCH_CELL + PATCH_MARGIN + (unit(hash >> 16) * span) as i32;
        let cz = gz * PATCH_CELL + PATCH_MARGIN + (unit(hash >> 32) * span) as i32;
        let (dx, dz) = (x - cx, z - cz);
        if dx.abs() > 2 || dz.abs() > 2 {
            return false;
        }
        let size = least + ((hash >> 48) % (most - least + 1) as u64) as u32;
        (dx, dz) == (0, 0)
            || (1..size).any(|k| {
                let h = hash3(seed, cx, k as i32, cz);
                ((h % 5) as i32 - 2, ((h >> 8) % 5) as i32 - 2) == (dx, dz)
            })
    }

    /// How well a plant grows on `ground` at a site with temperature `t` and soil moisture
    /// `m`, 0 ..= 1. Under snow, only plants with a snowy look grow, where they would on soil.
    fn grows(&self, plant: PlantId, site: &Site, ground: Ground, t: f32, m: f32) -> f32 {
        let spec = &self.plants[plant.0 as usize];
        if self.rooted(plant, ground) == 0.0 {
            return 0.0;
        }
        fit(&spec.comfort, t, m) * habitat(spec.habitat, site)
    }

    /// 1 if a plant can root in `ground`, else 0. Under snow, only plants with a snowy look
    /// root, where they would on soil.
    fn rooted(&self, plant: PlantId, ground: Ground) -> f32 {
        let spec = &self.plants[plant.0 as usize];
        let rooted = spec.ground & ground_bit(ground) != 0 || (ground == Ground::Snow && spec.snowy.is_some() && spec.ground & ground_bit(Ground::Soil) != 0);
        if rooted { 1.0 } else { 0.0 }
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
                let tags = content.plant_tags(plant, registry);
                // Plants that grow only where named keep to them (see `Flora::new`).
                if plant.named_only && !selects.iter().any(|s| matches!(s, Select::Name(n, _) if *n == plant.name)) {
                    return None;
                }
                let weight = selected(selects, &[], &plant.name, tags) * self.rarity[i];
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
    choose_with(list, total, u, |i| grows(list[i].0))
}

/// `choose`, with each entry's `grows` by its index in `list`.
fn choose_with(list: &[(PlantId, f32)], total: f32, u: f32, grows: impl Fn(usize) -> f32) -> PlantId {
    let mut left = u * total;
    let mut last = list[0].0;
    for (i, &(plant, weight)) in list.iter().enumerate() {
        let weight = weight * grows(i);
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
    /// `tree_rarity` and `plant_rarity` are the pools' rarities at the biome's level;
    /// `herb_rarity` the plants' own, for the rare herbs, whose rates are set per biome.
    fn new(biome: &BiomeDef, content: &WorldContent, registry: &BlockRegistry, tree_rarity: &[f32], plant_rarity: &[f32], herb_rarity: &[f32]) -> Self {
        let def = &biome.flora;
        // The palette: the colours of the biome's grass and foliage, the canopy's anchor
        // and the flowers' where data sets them.
        let [grass, foliage] = hue::biome_tints(content, biome);
        let rgb = |c: (u8, u8, u8)| [c.0 as f32, c.1 as f32, c.2 as f32];
        let flower_anchor = def.palette.flowers.map(rgb);
        let shown = |block: &str| hue::shown(registry.def(registry.expect(block)), grass, foliage);
        // How a tree's crown fits the palette. Tinted leaves take the place's foliage colour,
        // so they always fit; leaves of their own colour (cherry, maple, silverwood) grow
        // only in a biome whose palette is built around a colour like theirs, never as
        // specks among other colours.
        // A biome without a canopy colour of its own may still name such a tree (an olive's
        // grey-green, a larch's yellow-green): it grows as far as its colour sits with the
        // biome's foliage, judged more loosely, rarer the further it strays.
        let canopy_fit = |tree: &TreeDef, named: bool| -> f32 {
            let crown = tree.leaves.as_deref().unwrap_or(&tree.log);
            if !tree.crown.shades() || registry.def(registry.expect(crown)).tint.drawn() != voxmar_engine::block::Tint::None {
                return 1.0;
            }
            let fit = match def.palette.canopy {
                Some(anchor) => hue::harmony(shown(crown), rgb(anchor), hue::CANOPY_SPREAD),
                None if named => hue::harmony(shown(crown), foliage, hue::FOLIAGE_SPREAD),
                None => 0.0,
            };
            if fit < hue::CLASH { 0.0 } else { fit }
        };
        // Plants much brighter or more saturated than the grass are rarer accents.
        let contrast = |block: &str| 1.0 - hue::ACCENT_RARITY * (1.0 - hue::harmony(shown(block), grass, hue::CONTRAST_SPREAD));
        let wonders = def.wonders.clone().unwrap_or_else(|| vec![Select::Tag("wonder".into(), 1.0)]);
        let mut flora = Flora { forest: def.forest, cover: def.cover, flowers: def.flowers, bloom: def.bloom, name: crate::rarity::name_hash(&biome.name), ..Default::default() };
        for (i, tree) in content.trees.iter().enumerate() {
            let tags = tree.tags.iter().map(String::as_str);
            let id = TreeId(i as u16);
            // A tree the biome names is part of its forest, wonder or not.
            let named = def.trees.iter().any(|s| matches!(s, Select::Name(n, _) if *n == tree.name));
            let wonder = if named { 0.0 } else { selected(&wonders, &def.exclude, &tree.name, tags.clone()) * tree_rarity[i] * canopy_fit(tree, false) };
            if wonder > 0.0 {
                flora.wonder_trees.push((id, wonder));
                continue;
            }
            let weight = selected(&def.trees, &def.exclude, &tree.name, tags) * tree_rarity[i] * canopy_fit(tree, named);
            if weight > 0.0 {
                if tree.habitat.edge() { &mut flora.habitat } else { &mut flora.canopy }.push((id, weight));
            }
        }
        for (i, plant) in content.plants.iter().enumerate() {
            let tags = content.plant_tags(plant, registry);
            let id = PlantId(i as u16);
            // A plant that grows only where named (`named_only`, the rare herbs) keeps to the
            // biomes that name it, in their plants or wonders, so each keeps to its one or two.
            if plant.named_only {
                let named = |list: &[Select]| list.iter().find_map(|s| matches!(s, Select::Name(n, _) if *n == plant.name).then(|| s.weight())).unwrap_or(0.0);
                if plant.patches.is_some() {
                    let weight = named(&def.plants).max(def.wonders.as_deref().map_or(0.0, named));
                    if weight > 0.0 {
                        if plant.habitat.edge() { &mut flora.edge_herbs } else { &mut flora.patch_herbs }.push((id, weight));
                    }
                    continue;
                }
                // Rare herbs keep draft 89 rates independently of biome level.
                let wonder = def.wonders.as_deref().map_or(0.0, named) * herb_rarity[i];
                let weight = named(&def.plants) * herb_rarity[i];
                if wonder > 0.0 {
                    flora.wonder_plants.push((id, wonder));
                } else if weight > 0.0 {
                    match plant.spread {
                        Spread::Cover => flora.carpet.push((id, weight)),
                        Spread::Drifts => flora.drifts.push((id, weight)),
                        Spread::Alone => flora.alone.push((id, weight)),
                    }
                }
                continue;
            }
            let wonder = selected(&wonders, &def.exclude, &plant.name, tags.clone()) * plant_rarity[i] * contrast(&plant.block);
            if wonder > 0.0 {
                flora.wonder_plants.push((id, wonder));
                continue;
            }
            // Foraged plants (tagged `forage`) come only by name or by the `forage` tag, never
            // through a habitat tag such as `meadow`, so a biome takes them at the low weight
            // it gives the tag and they stay a rare find.
            let forage = tags.clone().any(|t| t == "forage");
            let mut weight = if forage {
                let own: Vec<Select> = def.plants.iter().filter(|s| matches!(s, Select::Name(..)) || matches!(s, Select::Tag(t, _) if t == "forage")).cloned().collect();
                selected(&own, &def.exclude, &plant.name, tags) * plant_rarity[i]
            } else {
                selected(&def.plants, &def.exclude, &plant.name, tags) * plant_rarity[i]
            };
            // Carpets keep to the grass's colour; flowers to their palette where one is set,
            // else, like plants that grow alone, the brightest against the grass are rarest.
            match (plant.spread, flower_anchor) {
                (Spread::Cover, _) => weight *= hue::harmony(shown(&plant.block), grass, hue::CARPET_SPREAD),
                (Spread::Drifts, Some(anchor)) => weight *= hue::harmony(shown(&plant.block), anchor, hue::FLOWER_SPREAD),
                (Spread::Drifts | Spread::Alone, _) => weight *= contrast(&plant.block),
            }
            if weight > 0.0 {
                match plant.spread {
                    Spread::Cover => flora.carpet.push((id, weight)),
                    Spread::Drifts => flora.drifts.push((id, weight)),
                    Spread::Alone => flora.alone.push((id, weight)),
                }
            }
        }
        // Flowers in order of hue, so the window of kinds a place grows holds neighbours.
        flora.drifts.sort_by(|a, b| {
            let hue_of = |p: PlantId| hue::hue(shown(&content.plants[p.0 as usize].block));
            hue_of(a.0).total_cmp(&hue_of(b.0)).then(a.0.0.cmp(&b.0.0))
        });
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
        crate::test_registry()
    }

    fn content(extra: &[(&str, &str)], registry: &BlockRegistry) -> WorldContent {
        let files: Vec<ContentFile> = BASE_FILES
            .iter()
            .map(|(name, source)| ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() })
            .chain(extra.iter().map(|(name, source)| ContentFile { package: "mod".into(), name: (*name).into(), source: (*source).into() }))
            .collect();
        WorldContent::from_files(files.iter(), registry).unwrap()
    }

    /// Each rare herb grows in the one or two biomes that name it and nowhere else: not
    /// through a tag, the forage sweep or the wonders every biome takes by default.
    #[test]
    fn rare_herbs_keep_to_the_biomes_that_name_them() {
        let registry = registry();
        let content = content(&[], &registry);
        let ecology = Ecology::new(1, &WorldDials::sample(1), &content, &registry);
        let rare: Vec<usize> = content.plants.iter().enumerate().filter(|(_, p)| p.named_only).map(|(i, _)| i).collect();
        assert!(rare.len() >= 10, "the ten rare herbs are tagged");
        for &i in &rare {
            let plant = &content.plants[i];
            let names = |list: &[Select]| list.iter().any(|s| matches!(s, Select::Name(n, _) if *n == plant.name));
            let mut grows_in = Vec::new();
            for (b, biome) in content.biomes.iter().enumerate() {
                let flora = &ecology.flora[b];
                let listed = [&flora.carpet, &flora.drifts, &flora.alone, &flora.wonder_plants, &flora.patch_herbs, &flora.edge_herbs].iter().any(|l| l.iter().any(|(p, _)| p.0 as usize == i));
                let named = names(&biome.flora.plants) || biome.flora.wonders.as_deref().is_some_and(names);
                assert_eq!(listed, named, "{} in {}: listed {listed}, named {named}", plant.name, biome.name);
                if listed {
                    grows_in.push(biome.name.clone());
                }
            }
            // A variant (alpine meadows in the mountains) counts with the biome it varies.
            let own = grows_in.iter().filter(|b| !content.biomes.iter().any(|p| grows_in.contains(&p.name) && p.variants.iter().any(|(v, _)| v == *b))).count();
            assert!((1..=2).contains(&own), "{} grows in {grows_in:?}", plant.name);
        }
    }

    /// A rare herb's patches make its rate before ground and climate thin them, in patches
    /// of 3 to 6 (design's drafts 61 and 89), cheaply, with no terrain: the full count in
    /// generated land, against each herb's aim, is `rare_herbs_grow_at_their_rates`.
    #[test]
    fn patches_make_a_rare_herbs_rate() {
        let registry = registry();
        let content = content(&[], &registry);
        let ecology = Ecology::new(1, &WorldDials::sample(1), &content, &registry);
        for (i, plant) in content.plants.iter().enumerate() {
            let Some(patches) = &plant.patches else { continue };
            assert!(plant.named_only, "{} grows in patches but not only where named", plant.name);
            if plant.habitat.edge() {
                continue;
            }
            assert!(patches.size.0 >= 3 && patches.size.1 <= 6 && patches.size.0 <= patches.size.1, "{}: size {:?}", plant.name, patches.size);
            let (span, mut hits, mut cells) = (4096, 0u32, std::collections::HashSet::new());
            for z in 0..span {
                for x in 0..span {
                    if ecology.in_patch(PlantId(i as u16), 1.0, x, z) {
                        hits += 1;
                        cells.insert((x.div_euclid(PATCH_CELL), z.div_euclid(PATCH_CELL)));
                    }
                }
            }
            let rate = hits as f32 * 1e6 / (span * span) as f32;
            let size = hits as f32 / cells.len().max(1) as f32;
            assert!((0.8 * patches.rate..=1.15 * patches.rate).contains(&rate), "{}: {rate:.0} per million, wants {}", plant.name, patches.rate);
            assert!((2.5..=6.0).contains(&size), "{}: {size:.1} a patch", plant.name);
        }
    }

    /// A rare herb of a lake's bank (the sky lotus) grows only on the bank, in patches of
    /// 3 to 6 that start from it, at its rate per column of bank, whatever the cell: here a
    /// straight bank two columns wide through cells of an alpine meadow.
    #[test]
    fn bank_herbs_grow_in_patches_along_their_bank() {
        let world = crate::Overworld::new(1, std::sync::Arc::new(registry()));
        let ecology = &world.ecology;
        let lotus = PlantId(ecology.plant_names.iter().position(|n| n == "base:sky_lotus").unwrap() as u16);
        let meadow = world.biomes.id("base:alpine_meadow").unwrap();
        let weight = ecology.flora[meadow.0 as usize].edge_herbs.iter().find(|(p, _)| *p == lotus).expect("alpine meadows grow sky lotus").1;
        let template = world.site(0, 0);
        let side = PATCH_CELL as usize;
        // Rows 14 and 15 of every cell are bank, a block over a lake's water; the rest is dry.
        let bank = |i: usize| (14..=15).contains(&(i / side));
        let sites: Vec<Site> = (0..side * side)
            .map(|i| {
                let (height, body) = if bank(i) { (201, crate::Body::Lake) } else { (210, crate::Body::Sea) };
                Site { height, water: 200, body, home: meadow, biome: meadow, forest: 0.0, ..template }
            })
            .collect();
        let (cells, mut plants, mut starts) = (2000, 0usize, 0usize);
        for c in 0..cells {
            let first = IVec2::new((c % 50) as i32 * PATCH_CELL, (c / 50) as i32 * PATCH_CELL);
            let grown = ecology.edge_patches(first, |i| Some((&sites[i], Ground::Soil)));
            assert!(grown.iter().all(|&(i, _)| bank(i)), "a sky lotus off the bank");
            // Patches: plants within reach of each other along the bank.
            let mut columns: Vec<i32> = grown.iter().map(|&(i, _)| (i % side) as i32).collect();
            columns.sort_unstable();
            starts += columns.windows(2).filter(|w| w[1] - w[0] > 2 * EDGE_REACH).count() + usize::from(!columns.is_empty());
            plants += grown.len();
        }
        let rate = plants as f32 * 1e6 / (cells * 2 * side) as f32;
        let wants = ecology.plants[lotus.0 as usize].patches.unwrap().0 * weight;
        assert!((0.8 * wants..=1.2 * wants).contains(&rate), "{rate:.0} per million columns of bank, wants {wants:.0}");
        let size = plants as f32 / starts.max(1) as f32;
        assert!((2.5..=6.5).contains(&size), "{size:.1} a patch");
    }

    /// Where a bank runs from one biome into another, each biome's stretch grows the herb at
    /// its own weight: a heavy neighbour's patches do not spill onto a light one's bank.
    /// Here the bank of the test above runs through alpine meadow (sky lotus 1.7) for the
    /// first half of each cell and snowy peaks (0.33) for the second.
    #[test]
    fn bank_herbs_keep_to_their_own_biome() {
        let world = crate::Overworld::new(1, std::sync::Arc::new(registry()));
        let ecology = &world.ecology;
        let lotus = PlantId(ecology.plant_names.iter().position(|n| n == "base:sky_lotus").unwrap() as u16);
        let biomes = [world.biomes.id("base:alpine_meadow").unwrap(), world.biomes.id("base:snowy_peaks").unwrap()];
        let weight = |b: crate::BiomeId| ecology.flora[b.0 as usize].edge_herbs.iter().find(|(p, _)| *p == lotus).expect("the biome grows sky lotus").1;
        let template = world.site(0, 0);
        let side = PATCH_CELL as usize;
        let bank = |i: usize| (14..=15).contains(&(i / side));
        let half = |i: usize| usize::from(i % side >= side / 2);
        let sites: Vec<Site> = (0..side * side)
            .map(|i| {
                let (height, body) = if bank(i) { (201, crate::Body::Lake) } else { (210, crate::Body::Sea) };
                let home = biomes[half(i)];
                Site { height, water: 200, body, home, biome: home, forest: 0.0, ..template }
            })
            .collect();
        let (cells, mut plants) = (4000, [0usize; 2]);
        for c in 0..cells {
            let first = IVec2::new((c % 64) as i32 * PATCH_CELL, (c / 64) as i32 * PATCH_CELL);
            for (i, _) in ecology.edge_patches(first, |i| Some((&sites[i], Ground::Soil))) {
                assert!(bank(i), "a sky lotus off the bank");
                plants[half(i)] += 1;
            }
        }
        let rate = ecology.plants[lotus.0 as usize].patches.unwrap().0;
        for (k, &biome) in biomes.iter().enumerate() {
            let per_million = plants[k] as f32 * 1e6 / (cells * side) as f32;
            let wants = rate * weight(biome);
            assert!((0.8 * wants..=1.2 * wants).contains(&per_million), "{}: {per_million:.0} per million columns of its bank, wants {wants:.0}", world.biomes.get(biome).name);
        }
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
    fn canopies_keep_to_their_palette() {
        let registry = registry();
        // A mod's two trees, both broadleaf: one green-leaved, one red.
        let tree = |name: &str, leaves: &str| {
            format!("(name: \"{name}\", tags: [\"broadleaf\", \"temperate\"], log: \"base:oak_log\", leaves: \"base:{leaves}\", height: (4, 7), crown: Round(reach: (1.5, 2.5), radius: (2.0, 2.8), squash: 0.8), comfort: (temperature: (7.0, 23.0)))")
        };
        let trees = format!("(trees: [{}, {}])", tree("green_ash", "oak_leaves"), tree("red_ash", "maple_leaves"));
        let content = content(&[("trees.ron", &trees)], &registry);
        let ecology = Ecology::new(1, &WorldDials::sample(1), &content, &registry);
        let share = |biome: &str, tree: &str| {
            let i = content.biomes.iter().position(|b| b.name == biome).unwrap();
            let canopy = &ecology.flora[i].canopy;
            let total: f32 = canopy.iter().map(|c| c.1).sum();
            let t = content.trees.iter().position(|t| t.name == tree).unwrap();
            canopy.iter().find(|c| c.0 .0 as usize == t).map_or(0.0, |c| c.1 / total)
        };
        // The forest takes broadleaf trees: the green one joins it, the red one hardly.
        assert!(share("base:forest", "mod:green_ash") > 0.1, "{}", share("base:forest", "mod:green_ash"));
        assert_eq!(share("base:forest", "mod:red_ash"), 0.0);
        assert_eq!(share("base:forest", "base:maple"), 0.0);
        // Biomes built around other colours keep to them.
        assert!(share("base:maple_forest", "base:amber_maple") > 0.2);
        assert!(share("base:cherry_grove", "base:cherry") > 0.8);
        assert_eq!(share("base:forest", "base:cherry"), 0.0);
    }

    #[test]
    fn gathering_trees_grow_only_where_named() {
        let registry = registry();
        let content = content(&[], &registry);
        let ecology = Ecology::new(1, &WorldDials::sample(1), &content, &registry);
        let name = |t: TreeId| content.trees[t.0 as usize].name.trim_start_matches("base:").to_owned();
        for (b, biome) in content.biomes.iter().enumerate() {
            let flora = &ecology.flora[b];
            for &(tree, _) in flora.canopy.iter().chain(&flora.habitat) {
                let def = &content.trees[tree.0 as usize];
                if def.tags.iter().any(|t| t == "gathering") {
                    let named = biome.flora.trees.iter().any(|s| matches!(s, Select::Name(n, _) if *n == def.name));
                    assert!(named, "{} grows in {} unnamed", def.name, biome.name);
                }
            }
        }
        let share = |biome: &str, tree: &str| {
            let flora = &ecology.flora[content.biomes.iter().position(|b| b.name == format!("base:{biome}")).unwrap()];
            let total: f32 = flora.canopy.iter().map(|c| c.1).sum();
            flora.canopy.iter().find(|c| name(c.0) == tree).map_or(0.0, |c| c.1 / total)
        };
        // Olives and larches, leaves of their own colour, grow where named though the
        // biome has no canopy colour, an accent rather than a stand.
        for (biome, tree) in [("mediterranean", "olive"), ("taiga", "larch"), ("bog", "larch")] {
            assert!((0.005..0.15).contains(&share(biome, tree)), "{tree} in {biome}: {}", share(biome, tree));
        }
        let habitat = |biome: &str| {
            let flora = &ecology.flora[content.biomes.iter().position(|b| b.name == format!("base:{biome}")).unwrap()];
            flora.habitat.iter().map(|c| name(c.0)).collect::<Vec<_>>()
        };
        assert!(habitat("bayou").contains(&"bald_cypress".to_owned()));
        assert!(habitat("swamp").contains(&"alder".to_owned()));
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

    /// A biome's level makes its rarer plants and gathering trees commoner by the levels
    /// table and keeps its common plants at two thirds above level 1 (P15), within two tiers
    /// of their own rarity; other trees keep their shares, and rare herbs the rates design
    /// set for their biome.
    #[test]
    fn a_biome_s_level_scales_its_finds() {
        let registry = registry();
        let content = content(&[], &registry);
        let mut higher = content.clone();
        let plains = content.biomes.iter().position(|b| b.name == "base:plains").unwrap();
        assert_eq!(content.biomes[plains].level, 1);
        higher.biomes[plains].level = 5;
        let dials = WorldDials::sample(1);
        let (low, high) = (Ecology::new(1, &dials, &content, &registry), Ecology::new(1, &dials, &higher, &registry));
        let levels = &content.climate.levels;
        let plant_class = |p: PlantId| {
            let def = &content.plants[p.0 as usize];
            def.rarity.unwrap_or_else(|| registry.def(registry.expect(&def.block)).rarity)
        };
        let (flora_low, flora_high) = (&low.flora[plains], &high.flora[plains]);
        let mut seen = [0; 5];
        for (a, b) in [(&flora_low.carpet, &flora_high.carpet), (&flora_low.drifts, &flora_high.drifts), (&flora_low.alone, &flora_high.alone)] {
            assert_eq!(a.len(), b.len());
            for (&(plant, w1), &(other, w5)) in a.iter().zip(b) {
                assert_eq!(plant, other);
                let class = plant_class(plant);
                let m = low.rarity[plant.0 as usize];
                let want = capped(class, m * levels.finds(5, class)) / capped(class, m);
                assert!((w5 / w1 - want).abs() < 1e-3 * want, "{} ({class:?}): {w1} at level 1, {w5} at level 5, wanted ×{want}", low.plant_names[plant.0 as usize]);
                seen[class as usize] += 1;
            }
        }
        // Plains grow commons and rarer finds alike, so both rules were tried.
        assert!(seen[0] > 0 && seen[1..].iter().sum::<i32>() > 0, "{seen:?}");
        let commons = flora_low.alone.iter().zip(&flora_high.alone).find(|((p, _), _)| plant_class(*p) == Rarity::Common);
        assert!(commons.is_some_and(|((_, w1), (_, w5))| (w5 / w1 - 0.67).abs() < 1e-3));
        // Gathering trees too; the canopy's own kinds keep their shares.
        let mut gathering = 0;
        for (&(tree, w1), &(_, w5)) in flora_low.canopy.iter().zip(&flora_high.canopy) {
            let def = &content.trees[tree.0 as usize];
            if def.tags.iter().any(|t| t == "gathering") {
                gathering += 1;
                // Rarer finds rise; a common one keeps two thirds, as the commons among plants do.
                let common = def.rarity == Rarity::Common;
                assert!(if common { (w5 / w1 - 0.67).abs() < 1e-3 } else { w5 > w1 }, "{} ({:?}): {w1} to {w5}", def.name, def.rarity);
            } else {
                assert_eq!(w5, w1, "{} keeps its share", def.name);
            }
        }
        assert!(gathering > 0 && gathering < flora_low.canopy.len(), "plains' forest has gathering trees and others");
        // A rare herb's biome may sit at any level; its rate stays as design set it.
        let herb = |p: &PlantId| content.plant_tags(&content.plants[p.0 as usize], &registry).any(|t| t == "rare_herb");
        for name in ["base:windroot", "base:sky_lotus"] {
            let home = content.biomes.iter().position(|b| b.flora.plants.iter().any(|s| matches!(s, Select::Name(n, _) if n == name))).expect("rare herb grows somewhere");
            let mut wild = content.clone();
            wild.biomes[home].level = 9;
            let wild = Ecology::new(1, &dials, &wild, &registry);
            let herbs = |e: &Ecology| {
                let f = &e.flora[home];
                [&f.alone, &f.carpet, &f.drifts, &f.wonder_plants, &f.patch_herbs, &f.edge_herbs]
                    .into_iter().flatten().filter(|(p, _)| herb(p)).copied().collect::<Vec<_>>()
            };
            let (a, b) = (herbs(&low), herbs(&wild));
            assert!(a.iter().any(|(p, _)| low.plant_names[p.0 as usize] == name), "{name} is in its biome's herb pool");
            assert_eq!(a, b, "{name}'s biome level keeps rare-herb rates");
        }
    }
}
