//! Ground decoration: patches of ground, rocks, bushes, tree ornaments, fallen logs and
//! stumps, from a shared pool defined as data (`decorations.ron`) that biomes select from
//! by tag or name, as they do trees and plants.
//!
//! Decorations are placed in a fixed order of stages (Geophilic's): patches of dirt, gravel
//! and moss first, then rocks, then trees and bushes, then the ornaments each tree makes of
//! its own blocks, then fallen logs and stumps; the plant stages and the snow blanket
//! follow in `terrain`. A later stage only fills what earlier ones left.
//!
//! Placed decorations share the trees' jittered grid of candidates: one roll per candidate
//! decides between a tree and the decorations, each taking a slice as wide as its density
//! there, so nothing overlaps at a candidate and nothing is counted per chunk (no density
//! steps along chunk borders). A decoration's density is its spacing, times the biome's
//! weight for it, its rarity, how well the climate suits it, and smooth ramps of fields it
//! follows: tree cover, the forest's stand, clearings, moisture, a named noise for clusters.
//! Patches take a second, independent roll, so they lie under trees too.
//!
//! Everything a feature decides comes from pure functions of position (the analytic
//! ground height, water, caves, the trees' own candidates), never from blocks another
//! column generated, so a log crossing a column border is laid identically by both. Each
//! checks its support from the same functions: a log lies only where most of it rests on
//! the ground and both its ends do, clear of trunks; rocks never sit over water or a cave.
//! Ornaments work from the blocks each tree put down (`TreeParts`): litter under the crown,
//! leaf heaps, fungi at the foot of the trunk.

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::{IVec2, IVec3, Vec3};

use crate::content::{Comfort, Field, Ground, Material, Select, Shape, WorldContent};
use crate::ecology::{Ecology, Plant, PlantId, fit, selected};
use crate::geology::{GeoGrid, smoothstep};
use crate::rarity::{Featured, Rarity, Tier};
use crate::rng::{Rng, hash3};
use crate::surface::{Palette, SurfaceRules};
use crate::terrain::Blocks;
use crate::trees::{TreeId, TreeParts, Trees};
use crate::{SEA_LEVEL, Site};

/// A decoration's index in the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecorId(pub u16);

/// The order decorations are placed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Patches,
    Rocks,
    Trees,
    Ornaments,
    Logs,
}

/// A field a density follows, compiled.
#[derive(Clone, Copy, Debug)]
enum Ramp {
    Forest,
    Stand,
    Clearing,
    Fertility,
    Moisture,
    Temperature,
    Slope,
    River,
    Altitude,
    Aether,
    Noise(usize),
}

/// Blocks for a decoration: chosen by weight, or the stones beneath the site.
#[derive(Clone, Debug)]
enum Mat {
    Blocks(Palette),
    Stones,
}

#[derive(Clone, Debug)]
enum Form {
    Patch { radius: (f32, f32), top: Mat },
    Boulder { radius: (f32, f32), stones: Mat, moss: f32 },
    Rocks { count: (i32, i32), spread: f32, size: (f32, f32), stones: Mat, moss: f32 },
    Bush { radius: (f32, f32), leaves: Option<BlockId>, log: Option<BlockId> },
    FallenLog { length: (i32, i32), thick: f32, hollow: f32, branches: f32, stump: f32, growth: f32, plants: Vec<(PlantId, f32)> },
    Stump { height: (i32, i32), growth: f32, plants: Vec<(PlantId, f32)> },
    Litter { top: Mat, cover: f32 },
    LeafPiles { cover: f32 },
    Foot { plants: Vec<(PlantId, f32)>, cover: f32 },
}

struct Decoration {
    form: Form,
    stage: Stage,
    /// Placed decorations: the chance per candidate cell where it is densest, before the
    /// biome's weight. Ornaments: the share of matching trees it decorates.
    chance: f32,
    follows: Vec<(Ramp, f32, f32)>,
    comfort: Comfort,
    /// Grounds it stands on, a bit each.
    ground: u8,
    /// Ornaments: weight per tree, by tree index.
    trees: Vec<f32>,
}

/// What one biome decorates its ground with: (decoration, chance) per kind of roll.
#[derive(Default)]
struct BiomeDecor {
    /// Decorations that take a candidate cell (rocks, bushes, logs), exclusive with trees.
    placed: Vec<(DecorId, f32)>,
    /// Patches of ground, on a roll of their own.
    patches: Vec<(DecorId, f32)>,
    ornaments: Vec<(DecorId, f32)>,
}

/// The decoration pool compiled for one world.
pub struct Decor {
    list: Vec<Decoration>,
    /// Per biome, in biome order.
    biomes: Vec<BiomeDecor>,
    moss: BlockId,
    seed: u64,
}

fn ground_bit(ground: Ground) -> u8 {
    1 << ground as u8
}

/// What placing decorations reads: the world, and the ground around the column.
pub struct Area<'a> {
    pub world: &'a crate::Overworld,
    pub sampler: &'a crate::Sampler<'a>,
    pub geology: &'a GeoGrid<'a>,
    pub caves: &'a crate::caves::CaveColumn<'a>,
    pub trees: &'a Trees,
}

/// What a density reads at a candidate besides its site.
pub struct Probe<'a> {
    pub rules: &'a SurfaceRules,
    pub x: i32,
    pub z: i32,
    /// The site's slope, which is costly and only asked for when a ramp needs it.
    pub slope: &'a mut dyn FnMut() -> i32,
}

impl Area<'_> {
    /// Whether nothing may stand on the ground at (x, z): water over it or a cave mouth.
    fn unfit(&self, x: i32, z: i32) -> bool {
        let geo = self.geology.get(x, z);
        geo.submerged() || self.caves.surface_open(x, z, &geo)
    }

    fn height(&self, x: i32, z: i32) -> i32 {
        self.geology.get(x, z).height
    }
}

impl Decor {
    pub fn new(
        content: &WorldContent,
        registry: &BlockRegistry,
        rules: &mut SurfaceRules,
        ecology: &Ecology,
        featured: &Featured,
        cell: i32,
        seed: u64,
    ) -> Self {
        let named: Vec<(&str, Rarity)> = content.decorations.iter().map(|d| (d.name.as_str(), d.rarity)).collect();
        let boost = featured.multipliers("decoration", &named);
        let plants = |selects: &[Select]| ecology.select(selects, content, registry);
        let mat = |m: &Material| match m {
            Material::Block(name) => Mat::Blocks(Palette::one(registry.expect(name))),
            Material::Mix(selects) => Mat::Blocks(Palette::new(selects, registry)),
            Material::Stones | Material::Strata => Mat::Stones,
        };
        let list: Vec<Decoration> = content
            .decorations
            .iter()
            .zip(&boost)
            .map(|(def, boost)| {
                let (form, stage) = match &def.shape {
                    Shape::Patch { radius, top } => (Form::Patch { radius: *radius, top: mat(top) }, Stage::Patches),
                    Shape::Boulder { radius, stones, moss } => (Form::Boulder { radius: *radius, stones: mat(stones), moss: *moss }, Stage::Rocks),
                    Shape::Rocks { count, spread, size, stones, moss } => {
                        (Form::Rocks { count: *count, spread: *spread, size: *size, stones: mat(stones), moss: *moss }, Stage::Rocks)
                    }
                    Shape::Bush { radius, leaves, log } => (
                        Form::Bush { radius: *radius, leaves: leaves.as_deref().map(|n| registry.expect(n)), log: log.as_deref().map(|n| registry.expect(n)) },
                        Stage::Trees,
                    ),
                    Shape::FallenLog { length, thick, hollow, branches, stump, growth, plants: list } => (
                        Form::FallenLog {
                            length: *length,
                            thick: *thick,
                            hollow: *hollow,
                            branches: *branches,
                            stump: *stump,
                            growth: *growth,
                            plants: plants(list),
                        },
                        Stage::Logs,
                    ),
                    Shape::Stump { height, growth, plants: list } => (Form::Stump { height: *height, growth: *growth, plants: plants(list) }, Stage::Logs),
                    Shape::Litter { top, cover } => (Form::Litter { top: mat(top), cover: *cover }, Stage::Ornaments),
                    Shape::LeafPiles { cover } => (Form::LeafPiles { cover: *cover }, Stage::Ornaments),
                    Shape::Foot { plants: list, cover } => (Form::Foot { plants: plants(list), cover: *cover }, Stage::Ornaments),
                };
                let rarity = def.rarity.multiplier() * boost;
                let chance = if def.ornament() {
                    def.chance.unwrap_or(1.0) * rarity
                } else {
                    let spacing = def.spacing.expect("checked at load");
                    ((cell as f32 / spacing).powi(2) * rarity).min(1.0)
                };
                let follows = def
                    .follows
                    .iter()
                    .map(|f| {
                        let ramp = match &f.0 {
                            Field::Forest => Ramp::Forest,
                            Field::Stand => Ramp::Stand,
                            Field::Clearing => Ramp::Clearing,
                            Field::Fertility => Ramp::Fertility,
                            Field::Moisture => Ramp::Moisture,
                            Field::Temperature => Ramp::Temperature,
                            Field::Slope => Ramp::Slope,
                            Field::River => Ramp::River,
                            Field::Altitude => Ramp::Altitude,
                            Field::Aether => Ramp::Aether,
                            Field::Noise(noise) => Ramp::Noise(rules.noise(noise)),
                        };
                        (ramp, f.1, f.2)
                    })
                    .collect();
                let trees = content.trees.iter().map(|t| selected(&def.trees, &[], &t.name, t.tags.iter().map(String::as_str))).collect();
                Decoration {
                    form,
                    stage,
                    chance,
                    follows,
                    comfort: def.comfort.clone(),
                    ground: def.grounds().iter().fold(0, |m, g| m | ground_bit(*g)),
                    trees,
                }
            })
            .collect();

        let biomes = content
            .biomes
            .iter()
            .map(|biome| {
                let selects = biome.flora.decorations.clone().unwrap_or_else(|| biome.tags.iter().map(|t| Select::Tag(t.clone(), 1.0)).collect());
                let mut out = BiomeDecor::default();
                for (i, def) in content.decorations.iter().enumerate() {
                    let weight = selected(&selects, &biome.flora.exclude, &def.name, def.tags.iter().map(String::as_str));
                    if weight <= 0.0 {
                        continue;
                    }
                    let entry = (DecorId(i as u16), weight * list[i].chance);
                    match list[i].stage {
                        Stage::Patches => out.patches.push(entry),
                        Stage::Ornaments => out.ornaments.push(entry),
                        _ => out.placed.push(entry),
                    }
                }
                // Rocks before bushes before logs, so a roll favours none of them by order.
                out.placed.sort_by_key(|&(id, _)| list[id.0 as usize].stage);
                out
            })
            .collect();
        Self { list, biomes, moss: registry.expect("base:moss_block"), seed: seed ^ 0xdec0 }
    }

    pub fn stage(&self, id: DecorId) -> Stage {
        self.list[id.0 as usize].stage
    }

    /// How dense a decoration is at a site relative to where it is densest, 0 ..= 1:
    /// its ramps times how well the climate suits it.
    fn density(&self, d: &Decoration, site: &Site, probe: &mut Probe) -> f32 {
        let mut density = fit(&d.comfort, site.temperature, site.soil_moisture());
        for &(ramp, from, to) in &d.follows {
            if density <= 0.0 {
                return 0.0;
            }
            let value = match ramp {
                Ramp::Forest => site.forest,
                Ramp::Stand => site.stand,
                Ramp::Clearing => site.clearing,
                Ramp::Fertility => site.fertility,
                Ramp::Moisture => site.soil_moisture(),
                Ramp::Temperature => site.temperature,
                Ramp::Slope => (probe.slope)() as f32,
                Ramp::River => site.river,
                Ramp::Altitude => (site.height - SEA_LEVEL) as f32,
                Ramp::Aether => site.aether,
                Ramp::Noise(i) => probe.rules.sample(i, probe.x, probe.z),
            };
            density *= if from < to { smoothstep(from, to, value) } else { 1.0 - smoothstep(to, from, value) };
        }
        density
    }

    /// The placed decoration a candidate's `roll` (already past the trees' share) lands
    /// on, if any, and the roll left over. The ground is not checked here.
    /// Each decoration of the list owns a slice of the roll as wide as its chance where it
    /// is densest, and is chosen when the roll falls in the part of its slice its density
    /// here fills, so decorations never take density from each other.
    fn choose(&self, list: &[(DecorId, f32)], site: &Site, mut roll: f32, probe: &mut Probe) -> Option<DecorId> {
        for &(id, chance) in list {
            if roll < chance && roll < chance * self.density(&self.list[id.0 as usize], site, probe) {
                return Some(id);
            }
            roll -= chance;
            if roll < 0.0 {
                return None;
            }
        }
        None
    }

    /// The placed decoration a candidate's `roll` (already past the trees' share) lands
    /// on, if any. The ground is not checked here.
    pub fn choose_placed(&self, site: &Site, roll: f32, probe: &mut Probe) -> Option<DecorId> {
        self.choose(&self.biomes[site.biome.0 as usize].placed, site, roll, probe)
    }

    /// The patch a candidate's own `roll` lands on, if any.
    pub fn choose_patch(&self, site: &Site, roll: f32, probe: &mut Probe) -> Option<DecorId> {
        self.choose(&self.biomes[site.biome.0 as usize].patches, site, roll, probe)
    }

    /// Whether a decoration stands on this ground.
    pub fn stands_on(&self, id: DecorId, ground: Ground) -> bool {
        self.list[id.0 as usize].ground & ground_bit(ground) != 0
    }

    /// Whether any of a biome's decorations take candidate cells.
    pub fn places_any(&self, site: &Site) -> bool {
        !self.biomes[site.biome.0 as usize].placed.is_empty()
    }

    pub fn has_patches(&self, site: &Site) -> bool {
        !self.biomes[site.biome.0 as usize].patches.is_empty()
    }

    /// Places a placed decoration or patch at `base`, the first block above the ground at
    /// its candidate, writing only the column's own blocks.
    #[allow(clippy::too_many_arguments)]
    pub fn place(&self, id: DecorId, area: &Area, site: &Site, base: IVec3, rng: &mut Rng, blocks: &mut Blocks, parts: &mut TreeParts) {
        match &self.list[id.0 as usize].form {
            Form::Patch { radius, top } => self.patch(area, site, base, *radius, top, rng, blocks),
            Form::Boulder { radius, stones, moss } => {
                let r = lerp(*radius, rng.unit());
                let stones = self.stones(area, site, stones);
                let block = stones.pick(rng.unit().min(0.9999));
                let centre = base.as_vec3() + Vec3::new(0.5, -0.35 + 0.1 * r, 0.5);
                let damp = *moss * dampness(site);
                self.rock(area, centre, Vec3::new(r, r * 0.8, r), block, damp, rng.next_u64(), blocks);
            }
            Form::Rocks { count, spread, size, stones, moss } => {
                let stones = self.stones(area, site, stones);
                let damp = *moss * dampness(site);
                for _ in 0..rng.range(count.0, count.1 + 1) {
                    let a = rng.unit() * std::f32::consts::TAU;
                    let d = spread * rng.unit().sqrt();
                    let (x, z) = ((base.x as f32 + 0.5 + a.cos() * d).floor() as i32, (base.z as f32 + 0.5 + a.sin() * d).floor() as i32);
                    let r = lerp(*size, rng.unit());
                    let block = stones.pick(rng.unit().min(0.9999));
                    let seed = rng.next_u64();
                    if area.unfit(x, z) || (area.height(x, z) - base.y + 1).abs() > 2 {
                        continue;
                    }
                    // Half sunk: the smallest lie flush with the ground.
                    let centre = Vec3::new(x as f32 + 0.5, area.height(x, z) as f32 + 0.2 + 0.5 * r, z as f32 + 0.5);
                    self.rock(area, centre, Vec3::new(r, r * 0.75, r), block, damp, seed, blocks);
                }
            }
            Form::Bush { radius, leaves, log } => {
                let species = Self::species(area, site, base, rng);
                let leaves = leaves.or_else(|| species.map(|t| area.trees.leaves(t))).filter(|b| !b.is_air());
                let Some(leaves) = leaves else { return };
                let log = log.or_else(|| species.map(|t| area.trees.log(t))).unwrap_or(BlockId::AIR);
                crate::trees::bush(base, lerp(*radius, rng.unit()), leaves, log, rng, blocks, parts);
            }
            Form::FallenLog { length, thick, hollow, branches, stump, growth, plants } => {
                let Some(species) = Self::species(area, site, base, rng) else { return };
                let log = Log {
                    length: rng.range(length.0, length.1 + 1),
                    axis: if rng.chance(0.5) { IVec2::X } else { IVec2::Y },
                    thick: area.trees.thick(species) || rng.chance(*thick),
                    roll: rng.unit(),
                };
                let hollow = log.thick && rng.chance(*hollow);
                let seed = rng.next_u64();
                if !log.fits(area, base) {
                    return;
                }
                self.lay(area, site, &log, base, species, hollow, *branches, *stump, *growth, plants, seed, blocks);
            }
            Form::Stump { height, growth, plants } => {
                let Some(species) = Self::species(area, site, base, rng) else { return };
                let height = rng.range(height.0, height.1 + 1);
                let seed = rng.next_u64();
                if Self::near_trunk(area, IVec2::new(base.x, base.z), IVec2::new(base.x, base.z), 1) {
                    return;
                }
                self.stump(area, site, base, species, height, *growth, plants, seed, blocks);
            }
            Form::Litter { .. } | Form::LeafPiles { .. } | Form::Foot { .. } => {}
        }
    }

    /// Decorates a tree with the ornaments of the biome at its base: `parts` holds what it
    /// put down in the column.
    pub fn ornament(&self, area: &Area, site: &Site, tree: TreeId, base: IVec3, parts: &TreeParts, blocks: &mut Blocks) {
        let list = &self.biomes[site.biome.0 as usize].ornaments;
        if list.is_empty() || (parts.leaves.is_empty() && parts.logs.is_empty()) {
            return;
        }
        let seed = hash3(self.seed, base.x, base.y, base.z);
        for (k, &(id, chance)) in list.iter().enumerate() {
            let d = &self.list[id.0 as usize];
            let weight = d.trees[tree.0 as usize];
            if weight <= 0.0 {
                continue;
            }
            let density = self.density(d, site, &mut Probe { rules: &area.world.rules, x: base.x, z: base.z, slope: &mut || site.slope });
            let roll = (hash3(seed, k as i32, 0, 0) & 0xffff) as f32 / 65536.0;
            if roll >= chance * weight.min(1.0) * density {
                continue;
            }
            let seed = hash3(seed, k as i32, 1, 0);
            match &d.form {
                Form::Litter { top, cover } => {
                    let top = self.stones(area, site, top);
                    for (x, z, near) in crown_ground(base, parts) {
                        let g = area.height(x, z);
                        let cell = IVec3::new(x, g, z);
                        let u = unit(hash3(seed, x, 0, z));
                        if u < cover * (1.0 - 0.55 * near) && self.soil(area, blocks, cell) {
                            blocks.set(cell, top.pick(unit(hash3(seed ^ 0x51, x, 0, z)).min(0.9999)));
                        }
                    }
                }
                Form::LeafPiles { cover } => {
                    let leaves = area.trees.leaves(tree);
                    for (x, z, near) in crown_ground(base, parts) {
                        let g = area.height(x, z);
                        let u = unit(hash3(seed, x, 0, z));
                        if u < cover * (1.0 - 0.5 * near) && self.soil(area, blocks, IVec3::new(x, g, z)) {
                            blocks.put_if_air(IVec3::new(x, g + 1, z), leaves);
                        }
                    }
                }
                Form::Foot { plants, cover } => {
                    let ecology = &area.world.ecology;
                    for cell in parts.logs.iter().filter(|c| c.y == base.y) {
                        for d in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
                            let at = *cell + d;
                            let u = unit(hash3(seed, at.x, at.y, at.z));
                            if u >= *cover || area.height(at.x, at.z) != base.y - 1 || !self.soil(area, blocks, at - IVec3::Y) {
                                continue;
                            }
                            if let Some(plant) = ecology.pick(plants, site, Ground::Soil, unit(hash3(seed ^ 0x77, at.x, 0, at.z)), u / cover) {
                                put_plant(blocks, at, plant);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether the block at `cell` is open soil an ornament may repaint or grow on.
    fn soil(&self, area: &Area, blocks: &Blocks, cell: IVec3) -> bool {
        let above = blocks.get(cell + IVec3::Y);
        area.world.terrain.ground(blocks.get(cell)) == Ground::Soil
            && (above.is_air() || area.world.terrain.is_plant(above))
            && !area.geology.get(cell.x, cell.z).submerged()
    }

    /// A decoration's blocks at a site.
    fn stones<'a>(&self, area: &'a Area, site: &Site, mat: &'a Mat) -> &'a Palette {
        match mat {
            Mat::Blocks(palette) => palette,
            Mat::Stones => area.world.stones(site),
        }
    }

    /// Repaints the ground in a ragged disc around `base`.
    #[allow(clippy::too_many_arguments)]
    fn patch(&self, area: &Area, site: &Site, base: IVec3, radius: (f32, f32), top: &Mat, rng: &mut Rng, blocks: &mut Blocks) {
        let r = lerp(radius, rng.unit());
        let seed = rng.next_u64();
        let top = self.stones(area, site, top);
        let reach = (r * 1.3).ceil() as i32;
        let Some((min, max)) = blocks.clip(base - IVec3::new(reach, 0, reach), base + IVec3::new(reach, 0, reach)) else { return };
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let (dx, dz) = ((x - base.x) as f32, (z - base.z) as f32);
                // A ragged edge: the radius wobbles with direction and from block to block.
                let wobble = 0.8 + 0.25 * (dx.atan2(dz) * 3.0 + unit(seed) * 6.0).sin() + 0.3 * unit(hash3(seed, x, 0, z));
                if dx * dx + dz * dz > (r * wobble).powi(2) {
                    continue;
                }
                let cell = IVec3::new(x, area.height(x, z), z);
                if (cell.y - base.y + 1).abs() <= 3 && self.soil(area, blocks, cell) {
                    blocks.set(cell, top.pick(unit(hash3(seed ^ 0x9e, x, 0, z)).min(0.9999)));
                }
            }
        }
    }

    /// An ellipsoid of rock, filling air and the top two blocks of the ground, so it rises
    /// out of the ground rather than sitting on it; in damp places, moss on the blocks of
    /// its top and its north face.
    #[allow(clippy::too_many_arguments)]
    fn rock(&self, area: &Area, centre: Vec3, radii: Vec3, block: BlockId, moss: f32, seed: u64, blocks: &mut Blocks) {
        let min = (centre - radii).floor().as_ivec3();
        let max = (centre + radii).floor().as_ivec3();
        let Some((min, max)) = blocks.clip(min, max) else { return };
        let inside = |cell: IVec3| {
            let d = (cell.as_vec3() + 0.5 - centre) / radii;
            d.length_squared() <= 1.0 - 0.3 * unit(hash3(seed, cell.x, cell.y, cell.z))
        };
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    let cell = IVec3::new(x, y, z);
                    if !inside(cell) {
                        continue;
                    }
                    // Moss grows on its top and, out of the sun, on its north face.
                    let top = !inside(cell + IVec3::Y);
                    let north = !inside(cell + IVec3::NEG_Z);
                    let shelter = if top { 1.0 } else if north { 0.7 } else { 0.0 };
                    let mossy = unit(hash3(seed ^ 0x3055, x, y, z)) < moss * shelter;
                    let rock = if mossy { self.moss } else { block };
                    let ground = area.height(x, z);
                    if y > ground {
                        blocks.put_if_air(cell, rock);
                    } else if y >= ground - 1
                        && matches!(area.world.terrain.ground(blocks.get(cell)), Ground::Soil | Ground::Sand | Ground::Snow)
                        && !area.geology.get(x, z).submerged()
                    {
                        blocks.set(cell, rock);
                    }
                }
            }
        }
    }

    /// The tree a decoration made of wood takes after: one of those growing at the site,
    /// chosen by their chances there; none where no trees grow.
    fn species(area: &Area, site: &Site, base: IVec3, rng: &mut Rng) -> Option<TreeId> {
        thread_local! {
            static CHANCES: std::cell::RefCell<Vec<(TreeId, f32)>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        let u = rng.unit();
        CHANCES.with_borrow_mut(|chances| {
            area.world.ecology.tree_chances(site, None, base.x, base.z, chances);
            // Shrubs, snags and cacti make no logs or bushes.
            chances.retain(|&(tree, _)| area.world.ecology.shades(tree));
            let total: f32 = chances.iter().map(|c| c.1).sum();
            let mut left = u * total;
            chances.iter().find(|&&(_, c)| {
                left -= c;
                left < 0.0
            })
            .map(|c| c.0)
        })
    }

    /// Whether a tree trunk stands within `margin` blocks of the box from `a` to `b`
    /// (inclusive, x and z), from the trees' own candidates.
    fn near_trunk(area: &Area, a: IVec2, b: IVec2, margin: i32) -> bool {
        let (lo, hi) = (a.min(b) - margin, a.max(b) + margin);
        let cell = crate::features::CELL;
        // A 2×2 trunk reaches one block towards −x and −z from its candidate.
        let (first, last) = ((lo).div_euclid(IVec2::splat(cell)), (hi + 1).div_euclid(IVec2::splat(cell)));
        for cz in first.y..=last.y {
            for cx in first.x..=last.x {
                if let Some(at) = area.world.terrain.features.tree_at(area, cx, cz)
                    && at.x >= lo.x - 1
                    && at.x <= hi.x
                    && at.y >= lo.y - 1
                    && at.y <= hi.y
                {
                    return true;
                }
            }
        }
        false
    }

    /// Lays a fallen trunk already found to fit.
    #[allow(clippy::too_many_arguments)]
    fn lay(
        &self,
        area: &Area,
        site: &Site,
        log: &Log,
        base: IVec3,
        species: TreeId,
        hollow: bool,
        branches: f32,
        stump: f32,
        growth: f32,
        plants: &[(PlantId, f32)],
        seed: u64,
        blocks: &mut Blocks,
    ) {
        let wood = area.trees.wood(species);
        let (axis, side) = (log.axis3(), log.side3());
        let start = log.start(base);
        let mut rng = Rng::new(seed, 0x106, 0, 0);
        let mut tops = Vec::new();
        for i in 0..log.length {
            let at = start + axis * i;
            if log.thick {
                // Three wide and three high, corners rounded off at random, hollow along
                // its middle if it rotted from the inside.
                for dy in 0..3 {
                    for ds in -1..=1 {
                        let corner = dy != 1 && ds != 0;
                        let cell = at + side * ds + IVec3::Y * dy;
                        if (corner && unit(hash3(seed, cell.x, cell.y, cell.z)) < 0.6) || (hollow && dy == 1 && ds == 0) {
                            continue;
                        }
                        blocks.put_log(cell, wood);
                    }
                }
                tops.push(at + IVec3::Y * 2);
            } else {
                blocks.put_log(at, wood);
                tops.push(at);
            }
        }
        // Branches: mostly sideways along the ground, some sticking up.
        if rng.chance(branches) {
            for _ in 0..rng.range(1, 3) {
                let i = rng.range(1, (log.length - 1).max(2));
                let at = start + axis * i;
                let length = rng.range(1, 3);
                if rng.chance(0.66) {
                    let out = if rng.chance(0.5) { side } else { -side };
                    let reach = if log.thick { 2 } else { 1 };
                    for k in 0..length {
                        let cell = at + out * (reach + k) + IVec3::Y * i32::from(log.thick);
                        if area.height(cell.x, cell.z) >= cell.y {
                            break;
                        }
                        blocks.put_log(cell, wood);
                    }
                } else {
                    let top = at + IVec3::Y * if log.thick { 3 } else { 1 };
                    for k in 0..length {
                        blocks.put_log(top + IVec3::Y * k, wood);
                    }
                }
            }
        }
        // The stump it broke from, a little way behind its foot.
        if rng.chance(stump) {
            let back = start - axis * rng.range(2, 4);
            let height = rng.range(1, 4);
            let g = area.height(back.x, back.z);
            let foot = IVec2::new(back.x, back.z);
            if !area.unfit(back.x, back.z) && (g + 1 - base.y).abs() <= 2 && !Self::near_trunk(area, foot, foot, 1) {
                self.stump(area, site, IVec3::new(back.x, g + 1, back.z), species, height, growth, plants, rng.next_u64(), blocks);
            }
        }
        // Fungi, ferns and moss-loving plants on top.
        if growth > 0.0 {
            let ecology = &area.world.ecology;
            for top in tops {
                let u = unit(hash3(seed ^ 0x9407, top.x, top.y, top.z));
                if u < growth
                    && let Some(plant) = ecology.pick(plants, site, Ground::Soil, unit(hash3(seed ^ 0x51, top.x, 0, top.z)), u / growth)
                {
                    put_plant(blocks, top + IVec3::Y, plant);
                }
            }
        }
    }

    /// A stump `height` tall standing on the ground at `base`, maybe with a plant on top.
    #[allow(clippy::too_many_arguments)]
    fn stump(&self, area: &Area, site: &Site, base: IVec3, species: TreeId, height: i32, growth: f32, plants: &[(PlantId, f32)], seed: u64, blocks: &mut Blocks) {
        let log = area.trees.log(species);
        for y in 0..height {
            blocks.put_log(base + IVec3::Y * y, log);
        }
        let u = unit(seed);
        if u < growth
            && let Some(plant) = area.world.ecology.pick(plants, site, Ground::Soil, unit(seed >> 24), u / growth)
        {
            put_plant(blocks, base + IVec3::Y * height, plant);
        }
    }
}

/// A fallen trunk's layout.
struct Log {
    length: i32,
    /// Along x (`X`) or z (`Y`).
    axis: IVec2,
    thick: bool,
    /// Which way along its axis it is offset.
    roll: f32,
}

impl Log {
    fn axis3(&self) -> IVec3 {
        IVec3::new(self.axis.x, 0, self.axis.y)
    }

    fn side3(&self) -> IVec3 {
        IVec3::new(self.axis.y, 0, self.axis.x)
    }

    /// The cell of its foot, given the candidate's `base`: it lies centred on the base.
    fn start(&self, base: IVec3) -> IVec3 {
        let back = if self.roll < 0.5 { self.length / 2 } else { (self.length - 1) / 2 };
        base - self.axis3() * back
    }

    /// Whether it may lie here: dry ground with no cave mouth under all of it, none of it
    /// buried, most of its length and both its ends resting on the ground, and no trunk
    /// beside it.
    fn fits(&self, area: &Area, base: IVec3) -> bool {
        let (axis, side) = (self.axis3(), self.side3());
        let start = self.start(base);
        let half = i32::from(self.thick);
        let (mut cells, mut resting) = (0, 0);
        for i in 0..self.length {
            let end = i == 0 || i == self.length - 1;
            let mut end_rests = false;
            for ds in -half..=half {
                let at = start + axis * i + side * ds;
                if area.unfit(at.x, at.z) {
                    return false;
                }
                let g = area.height(at.x, at.z);
                if g >= base.y {
                    return false;
                }
                cells += 1;
                if g == base.y - 1 {
                    resting += 1;
                    end_rests = true;
                }
            }
            if end && !end_rests {
                return false;
            }
        }
        if resting * 10 < cells * 6 {
            return false;
        }
        let (a, b) = (start - side * half, start + axis * (self.length - 1) + side * half);
        !Decor::near_trunk(area, IVec2::new(a.x, a.z), IVec2::new(b.x, b.z), 1)
    }
}

/// Puts a small plant at `at`, where it and the cells it needs above are open.
fn put_plant(blocks: &mut Blocks, at: IVec3, plant: Plant) {
    let open = |blocks: &Blocks, p: IVec3| blocks.get(p).is_air();
    match plant {
        Plant::One(block) => {
            blocks.put_if_air(at, block);
        }
        Plant::Tall(bottom, top) => {
            if open(blocks, at) && open(blocks, at + IVec3::Y) {
                blocks.set(at, bottom);
                blocks.set(at + IVec3::Y, top);
            }
        }
        Plant::Stack(block, n) => {
            for y in 0..n {
                if !blocks.put_if_air(at + IVec3::Y * y, block) {
                    break;
                }
            }
        }
    }
}

/// The ground positions under a tree's crown in the column, once each, with how near the
/// trunk each lies (0 at it, 1 at the crown's edge): from its leaves, and the ring around
/// its foot.
fn crown_ground(base: IVec3, parts: &TreeParts) -> Vec<(i32, i32, f32)> {
    let mut seen: Vec<(i32, i32)> = parts.leaves.iter().chain(&parts.logs).map(|c| (c.x, c.z)).collect();
    seen.sort_unstable();
    seen.dedup();
    let reach = seen.iter().map(|&(x, z)| ((x - base.x).pow(2) + (z - base.z).pow(2)) as f32).fold(4.0f32, f32::max).sqrt();
    seen.into_iter().map(|(x, z)| (x, z, (((x - base.x).pow(2) + (z - base.z).pow(2)) as f32).sqrt() / reach)).collect()
}

/// How damp and shaded a site is, for moss: 0 ..= 1.
fn dampness(site: &Site) -> f32 {
    smoothstep(0.75, 1.25, site.soil_moisture()) * (0.35 + 0.65 * site.forest)
}

fn lerp((lo, hi): (f32, f32), t: f32) -> f32 {
    lo + (hi - lo) * t
}

fn unit(bits: u64) -> f32 {
    (bits & 0xff_ffff) as f32 / (1u32 << 24) as f32
}
