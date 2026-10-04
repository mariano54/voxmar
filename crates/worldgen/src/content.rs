//! World content: the generator's definitions, read from content packages the way blocks
//! are, so a mod adds a biome, a tree or a rock province exactly as the base game does.
//!
//! Each package may hold `climate.ron` (the bands every biome is placed by, and the blocks
//! of snow and ice), `rocks.ron` (rock provinces), `biomes.ron`, `trees.ron` and
//! `plants.ron`. Names are qualified with the package (`base:taiga`). Loading resolves
//! `inherit`, then checks every reference: blocks, provinces, biomes, trees, plants, band
//! names, and that the biome table has no holes. A broken package fails at load with a
//! message naming the file, the definition and what is wrong, never later in generation.

use std::fmt;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use voxmar_engine::block::BlockRegistry;
use voxmar_engine::content::{Content, ContentFile};

use crate::rarity::Rarity;

#[derive(Debug, PartialEq)]
pub struct ContentError(pub String);

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ContentError {}

/// A choice from a shared pool, with a weight: one definition by name, or every
/// definition carrying a tag. Biomes list what they draw on this way, so a block, tree or
/// plant a mod adds with the right tags appears in every fitting biome.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum Select {
    Name(String, f32),
    Tag(String, f32),
}

impl Select {
    pub fn weight(&self) -> f32 {
        match self {
            Select::Name(_, weight) | Select::Tag(_, weight) => *weight,
        }
    }
}

// ---------------------------------------------------------------------------------------
// climate.ron

/// The bands biomes are placed by, and the blocks of snow and ice.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClimateDef {
    /// Temperature at the regional elevation, °C.
    pub temperature: BandsDef,
    /// Precipitation over potential evaporation.
    pub moisture: BandsDef,
    /// −1 deep ocean ..= 1 deep inland.
    pub continentalness: BandsDef,
    /// The landform class, 0 flat ..= 1 peaks.
    pub erosion: BandsDef,
    /// Monotone curves that bend an axis for the biome picker only (Terralith's remaps):
    /// stretching a tail gives a rare slot more land without changing the terrain.
    #[serde(default)]
    pub remaps: Vec<RemapDef>,
    pub frost: FrostDef,
}

/// Named bands along one axis: `names[i]` lies below `edges[i]` and above `edges[i - 1]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandsDef {
    pub names: Vec<String>,
    pub edges: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Axis {
    Temperature,
    Moisture,
    Continentalness,
    Erosion,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemapDef {
    pub axis: Axis,
    /// (from, to) points, increasing in both.
    pub points: Vec<(f32, f32)>,
}

/// What frost makes of the ground and of still water.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrostDef {
    /// Deep snow.
    pub snow: String,
    /// The softest snow, where it is coldest.
    pub powder: String,
    /// Ground under a thin cover of snow.
    pub snowy_ground: String,
    pub ice: String,
    /// Ice where it is very cold.
    pub thick_ice: String,
    /// A rare block among thick ice.
    pub rare_ice: String,
}

// ---------------------------------------------------------------------------------------
// rocks.ron

/// A rock province: a region kilometres across with its own stones, independent of the
/// climate, so a desert may lie on sandstone or on granite.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvinceDef {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// How much of the world it covers, relative to other provinces.
    #[serde(default = "one")]
    pub weight: f32,
    #[serde(default)]
    pub rarity: Rarity,
    /// The stones of the province and their shares, underground and on bare rock.
    pub stones: Vec<Select>,
}

impl ProvinceDef {
    /// Whether a province condition names this province (`granite` or `base:granite`) or
    /// one of its tags.
    pub fn matches(&self, name: &str) -> bool {
        self.name == name || self.name.split_once(':').is_some_and(|(_, local)| local == name) || self.tags.iter().any(|t| t == name)
    }
}

// ---------------------------------------------------------------------------------------
// biomes.ron

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Layer {
    /// The ground under the sky.
    #[default]
    Surface,
    /// Caves, chosen by depth as well (later).
    Underground,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Role {
    /// Claims the land and sea of its climate box.
    #[default]
    Land,
    /// Rivers and lakes within its climate box, wherever fresh water lies.
    River,
}

/// The part of the biome table a biome claims: a range of bands on each axis (all of
/// them when left out), and optionally a range of aether.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ClimateBox {
    pub temperature: Option<(String, String)>,
    pub moisture: Option<(String, String)>,
    pub continentalness: Option<(String, String)>,
    pub erosion: Option<(String, String)>,
    /// Where aether is in this range the biome displaces the ordinary biomes of its cells.
    pub aether: Option<(f32, f32)>,
}

impl ClimateBox {
    pub fn bands(&self, axis: Axis) -> Option<&(String, String)> {
        match axis {
            Axis::Temperature => self.temperature.as_ref(),
            Axis::Moisture => self.moisture.as_ref(),
            Axis::Continentalness => self.continentalness.as_ref(),
            Axis::Erosion => self.erosion.as_ref(),
        }
    }
}

/// A thin slice along the border of two biomes, across one axis (Wilder Wild's ecotones):
/// birch taiga between taiga and forest.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EcotoneDef {
    pub between: (String, String),
    #[serde(default = "temperature_axis")]
    pub axis: Axis,
    /// Width of the slice in the axis' units (°C across temperature).
    pub width: f32,
}

/// Blocks for one part of the surface.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum Material {
    Block(String),
    /// Several blocks in patches, by weight.
    Mix(Vec<Select>),
    /// The stones beneath: the biome's or the rock province's.
    Stones,
    /// The biome's `strata`: horizontal bands that repeat up the world.
    Strata,
}

/// The top block of a column and what lies under it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayersDef {
    pub top: Material,
    /// Beneath the top; the top itself when left out.
    #[serde(default)]
    pub filler: Option<Material>,
    /// Blocks of filler, 3 when left out.
    #[serde(default)]
    pub depth: Option<i32>,
}

/// A 2D noise field named for its seed, so rules naming the same field share it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoiseDef {
    pub name: String,
    /// Typical feature size in blocks.
    pub scale: f32,
    #[serde(default = "one_octave")]
    pub octaves: i32,
}

/// A test on a column. Thresholds given as two values are dithered between them in
/// patches, so the borders they draw are ragged rather than lines.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum Condition {
    /// A seabed, lake or river floor. Rules with this condition apply only under water;
    /// the others only on dry land.
    Underwater,
    /// Under more than this many blocks of water.
    Deeper(f32, f32),
    /// The water over the column, or the nearest beside it, is one of these: `Creek`,
    /// `River`, `Lake`, `Salt` (a dry salt flat) or `Sea`. For banks and beds that differ
    /// by kind of water.
    Water(Vec<crate::Body>),
    /// Steeper than this many blocks per block.
    Slope(f32, f32),
    /// Higher than this many blocks above the sea.
    Altitude(f32, f32),
    /// Colder than this, °C.
    Colder(f32, f32),
    Warmer(f32, f32),
    /// Soil moisture above this.
    Wetter(f32, f32),
    Drier(f32, f32),
    /// Tree cover above this, 0 ..= 1.
    Forest(f32, f32),
    /// Nearness to a river or lake above this, 0 ..= 1.
    River(f32, f32),
    /// The rock province, by name (`granite` or `base:granite`) or tag.
    Province(String),
    /// `|noise| < width`: thin lines along the noise's zero contour (trails, creeks).
    Band(NoiseDef, f32),
    /// Like `Band`, with the width chosen by a second noise: the first width whose
    /// threshold the second noise reaches, from a list of (threshold, width) in descending
    /// thresholds. Lines that taper (mineral rivulets, sediment seams).
    Tapered(NoiseDef, NoiseDef, Vec<(f32, f32)>),
    /// Where the noise is above this: patches.
    Patch(NoiseDef, f32),
    /// At random, with this chance per column.
    Chance(f32),
    Not(Box<Condition>),
    Any(Vec<Condition>),
}

/// When all conditions hold, these layers make the column.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    pub when: Vec<Condition>,
    pub place: LayersDef,
}

/// How a biome's ground is made, column by column. Under water: the first rule marked
/// `Underwater` whose conditions hold, else `underwater`. On dry land: `cliff` on steep
/// slopes; snow where frost lies; `cliff` again on bare high ground; the first other rule
/// whose conditions hold; `shore` on beaches and banks; otherwise `ground`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceDef {
    #[serde(default)]
    pub rules: Vec<RuleDef>,
    pub ground: LayersDef,
    pub underwater: LayersDef,
    #[serde(default)]
    pub shore: Option<LayersDef>,
    /// The stones beneath when left out.
    #[serde(default)]
    pub cliff: Option<Material>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub enum Stones {
    /// The stones of the rock province.
    #[default]
    Province,
    /// Stones of this biome's own.
    Blocks(Vec<Select>),
}

/// Colours an identity biome sets for itself. Ordinary biomes leave them out and take
/// the continuous climate colours, so their borders show no seams.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Colors {
    pub grass: Option<(u8, u8, u8)>,
    pub foliage: Option<(u8, u8, u8)>,
    pub water: Option<(u8, u8, u8)>,
}

/// A biome as written, every field optional so that `inherit` can fill it in.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawBiome {
    name: String,
    inherit: Option<String>,
    tags: Option<Vec<String>>,
    layer: Option<Layer>,
    role: Option<Role>,
    climate: Option<Vec<ClimateBox>>,
    ecotone: Option<EcotoneDef>,
    weight: Option<f32>,
    rarity: Option<Rarity>,
    surface: Option<SurfaceDef>,
    stones: Option<Stones>,
    strata: Option<Vec<String>>,
    colors: Option<Colors>,
    variants: Option<Vec<(String, f32)>>,
}

/// A biome with its inheritance resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct BiomeDef {
    pub name: String,
    pub tags: Vec<String>,
    pub layer: Layer,
    pub role: Role,
    /// The cells it claims; empty for ecotones and biomes that only grow as variants.
    pub climate: Vec<ClimateBox>,
    pub ecotone: Option<EcotoneDef>,
    /// Its share of the cells it claims, against other claimants.
    pub weight: f32,
    pub rarity: Rarity,
    pub surface: SurfaceDef,
    pub stones: Stones,
    /// Blocks of banded rock, one block each, bottom to top, repeating up the world.
    pub strata: Vec<String>,
    pub colors: Colors,
    /// Sub-biomes and the share of this biome's land each takes.
    pub variants: Vec<(String, f32)>,
}

// ---------------------------------------------------------------------------------------
// Loading

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvinceFile {
    provinces: Vec<ProvinceDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BiomeFile {
    biomes: Vec<RawBiome>,
}

/// Everything the world generator reads from content packages.
#[derive(Clone, Debug)]
pub struct WorldContent {
    pub climate: ClimateDef,
    pub provinces: Vec<ProvinceDef>,
    pub biomes: Vec<BiomeDef>,
}

/// The base package's files, built in so that a generator can be made from a block
/// registry alone (distant terrain, tools and tests).
pub(crate) const BASE_FILES: [(&str, &str); 3] = [
    ("biomes.ron", include_str!("../../../assets/content/base/biomes.ron")),
    ("climate.ron", include_str!("../../../assets/content/base/climate.ron")),
    ("rocks.ron", include_str!("../../../assets/content/base/rocks.ron")),
];

impl WorldContent {
    /// The generator's definitions from loaded content packages, checked against their
    /// blocks.
    pub fn load(content: &Content) -> Result<Self, ContentError> {
        Self::from_files(content.files.iter(), &content.blocks)
    }

    /// The base package's definitions alone.
    pub fn base(registry: &BlockRegistry) -> Result<Self, ContentError> {
        let files: Vec<ContentFile> =
            BASE_FILES.iter().map(|(name, source)| ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() }).collect();
        Self::from_files(files.iter(), registry)
    }

    /// Parses definition files given in package order. A later package's `climate.ron`
    /// replaces an earlier one's; other definitions accumulate.
    pub fn from_files<'a>(files: impl Iterator<Item = &'a ContentFile>, registry: &BlockRegistry) -> Result<Self, ContentError> {
        let mut climate = None;
        let mut provinces = Vec::new();
        let mut raw = Vec::new();
        for file in files {
            let at = format!("{}/{}", file.package, file.name);
            let q = |name: &str| qualify(&file.package, name);
            match file.name.as_str() {
                "climate.ron" => {
                    let def = parse::<ClimateDef>(&at, &file.source)?;
                    let f = &def.frost;
                    let frost = FrostDef {
                        snow: q(&f.snow),
                        powder: q(&f.powder),
                        snowy_ground: q(&f.snowy_ground),
                        ice: q(&f.ice),
                        thick_ice: q(&f.thick_ice),
                        rare_ice: q(&f.rare_ice),
                    };
                    climate = Some(ClimateDef { frost, ..def });
                }
                "rocks.ron" => provinces.extend(parse::<ProvinceFile>(&at, &file.source)?.provinces.into_iter().map(|p| ProvinceDef {
                    name: q(&p.name),
                    stones: p.stones.iter().map(|s| qualify_select(&file.package, s)).collect(),
                    ..p
                })),
                "biomes.ron" => {
                    for biome in parse::<BiomeFile>(&at, &file.source)?.biomes {
                        raw.push((at.clone(), qualify_biome(&file.package, biome)));
                    }
                }
                _ => {}
            }
        }
        let climate = climate.ok_or_else(|| ContentError("no content package defines climate.ron".into()))?;
        let biomes = inherit(&raw)?;
        let content = Self { climate, provinces, biomes };
        content.check(registry, &raw.iter().map(|(at, b)| (b.name.clone(), at.clone())).collect::<Vec<_>>())?;
        Ok(content)
    }

    pub fn biome(&self, name: &str) -> Option<&BiomeDef> {
        self.biomes.iter().find(|b| b.name == name)
    }

    /// The bands of one axis.
    pub fn bands(&self, axis: Axis) -> &BandsDef {
        match axis {
            Axis::Temperature => &self.climate.temperature,
            Axis::Moisture => &self.climate.moisture,
            Axis::Continentalness => &self.climate.continentalness,
            Axis::Erosion => &self.climate.erosion,
        }
    }

    /// Checks every reference and the biome table's coverage.
    fn check(&self, registry: &BlockRegistry, files: &[(String, String)]) -> Result<(), ContentError> {
        let file_of = |name: &str| files.iter().find(|(n, _)| n == name).map_or("?", |(_, at)| at.as_str()).to_owned();
        let block = |at: &dyn Fn() -> String, name: &str| {
            registry.id(name).map(|_| ()).ok_or_else(|| ContentError(format!("{}: unknown block `{name}`", at())))
        };
        let blocks = |at: &dyn Fn() -> String, selects: &[Select]| -> Result<(), ContentError> {
            if selects.is_empty() {
                return Err(ContentError(format!("{}: an empty list of blocks", at())));
            }
            for select in selects {
                match select {
                    Select::Name(name, weight) => {
                        block(at, name)?;
                        positive(at, "weight", *weight)?;
                    }
                    Select::Tag(tag, weight) => {
                        if registry.tagged(tag).next().is_none() {
                            return Err(ContentError(format!("{}: no block has the tag `{tag}`", at())));
                        }
                        positive(at, "weight", *weight)?;
                    }
                }
            }
            Ok(())
        };

        // Climate.
        let climate_at = || "climate.ron".to_owned();
        for axis in [Axis::Temperature, Axis::Moisture, Axis::Continentalness, Axis::Erosion] {
            let bands = self.bands(axis);
            if bands.names.is_empty() || bands.edges.len() + 1 != bands.names.len() {
                return Err(ContentError(format!("climate.ron: {axis:?} needs one more band name than edges")));
            }
            if bands.edges.windows(2).any(|w| w[0] >= w[1]) || bands.edges.iter().any(|e| !e.is_finite()) {
                return Err(ContentError(format!("climate.ron: {axis:?} band edges must increase")));
            }
        }
        for remap in &self.climate.remaps {
            if remap.points.len() < 2 || remap.points.windows(2).any(|w| !(w[0].0 < w[1].0 && w[0].1 <= w[1].1)) {
                return Err(ContentError(format!("climate.ron: the {:?} remap must have increasing points", remap.axis)));
            }
        }
        let frost = &self.climate.frost;
        for name in [&frost.snow, &frost.powder, &frost.snowy_ground, &frost.ice, &frost.thick_ice, &frost.rare_ice] {
            block(&climate_at, name)?;
        }

        // Provinces.
        if self.provinces.is_empty() {
            return Err(ContentError("no content package defines a rock province".into()));
        }
        for (i, province) in self.provinces.iter().enumerate() {
            let at = || format!("rocks.ron: province `{}`", province.name);
            if self.provinces[..i].iter().any(|p| p.name == province.name) {
                return Err(ContentError(format!("{}: defined twice", at())));
            }
            positive(&at, "weight", province.weight)?;
            blocks(&at, &province.stones)?;
        }

        // Biomes.
        let surface_biomes = || self.biomes.iter().filter(|b| b.layer == Layer::Surface);
        for (i, biome) in self.biomes.iter().enumerate() {
            let file = file_of(&biome.name);
            let at = || format!("{file}: biome `{}`", biome.name);
            if self.biomes[..i].iter().any(|b| b.name == biome.name) {
                return Err(ContentError(format!("{}: defined twice", at())));
            }
            positive(&at, "weight", biome.weight)?;
            for climate in &biome.climate {
                for axis in [Axis::Temperature, Axis::Moisture, Axis::Continentalness, Axis::Erosion] {
                    if let Some((from, to)) = climate.bands(axis) {
                        let names = &self.bands(axis).names;
                        let index = |band: &str| {
                            names.iter().position(|n| n == band).ok_or_else(|| {
                                ContentError(format!("{}: unknown {axis:?} band `{band}` (bands: {})", at(), names.join(", ")))
                            })
                        };
                        if index(from)? > index(to)? {
                            return Err(ContentError(format!("{}: the {axis:?} range from `{from}` to `{to}` is empty", at())));
                        }
                    }
                }
                if let Some((lo, hi)) = climate.aether
                    && !(0.0 <= lo && lo < hi && hi <= 1.0)
                {
                    return Err(ContentError(format!("{}: the aether range ({lo}, {hi}) is empty or outside 0..=1", at())));
                }
            }
            let is_variant = self.biomes.iter().any(|b| b.variants.iter().any(|(v, _)| *v == biome.name));
            if biome.layer == Layer::Surface && biome.climate.is_empty() && biome.ecotone.is_none() && !is_variant {
                return Err(ContentError(format!("{}: never grows: it has no climate, is no ecotone and no biome's variant", at())));
            }
            if let Some(ecotone) = &biome.ecotone {
                if ecotone.width.is_nan() || ecotone.width <= 0.0 {
                    return Err(ContentError(format!("{}: the ecotone needs a positive width", at())));
                }
                if !matches!(ecotone.axis, Axis::Temperature | Axis::Moisture) {
                    return Err(ContentError(format!("{}: ecotones run across temperature or moisture", at())));
                }
                for parent in [&ecotone.between.0, &ecotone.between.1] {
                    if !surface_biomes().any(|b| b.name == *parent && b.role == Role::Land) {
                        return Err(ContentError(format!("{}: the ecotone's parent `{parent}` is no land biome", at())));
                    }
                }
            }
            let mut shares = 0.0;
            for (variant, share) in &biome.variants {
                let Some(def) = self.biome(variant) else {
                    return Err(ContentError(format!("{}: unknown variant `{variant}`", at())));
                };
                if !def.variants.is_empty() {
                    return Err(ContentError(format!("{}: the variant `{variant}` has variants of its own", at())));
                }
                if !(*share > 0.0 && *share <= 1.0) {
                    return Err(ContentError(format!("{}: the share of `{variant}` must be in (0, 1]", at())));
                }
                shares += share;
            }
            if shares > 1.0 + 1e-4 {
                return Err(ContentError(format!("{}: its variants' shares add up to more than 1", at())));
            }
            if let Stones::Blocks(stones) = &biome.stones {
                blocks(&|| format!("{}, stones", at()), stones)?;
            }
            for name in &biome.strata {
                block(&|| format!("{}, strata", at()), name)?;
            }
            self.check_surface(&biome.surface, !biome.strata.is_empty(), &|| at(), &block, &blocks)?;
        }

        self.check_coverage()
    }

    fn check_surface(
        &self,
        surface: &SurfaceDef,
        strata: bool,
        at: &dyn Fn() -> String,
        block: &BlockCheck<'_, str>,
        blocks: &BlockCheck<'_, [Select]>,
    ) -> Result<(), ContentError> {
        let material = |part: &str, material: &Material| -> Result<(), ContentError> {
            let at = || format!("{}, surface {part}", at());
            match material {
                Material::Block(name) => block(&at, name),
                Material::Mix(selects) => blocks(&at, selects),
                Material::Stones => Ok(()),
                Material::Strata if strata => Ok(()),
                Material::Strata => Err(ContentError(format!("{}: uses strata but the biome has none", at()))),
            }
        };
        let layers = |part: &str, layers: &LayersDef| -> Result<(), ContentError> {
            material(part, &layers.top)?;
            if let Some(filler) = &layers.filler {
                material(part, filler)?;
            }
            match layers.depth {
                Some(depth) if !(0..=24).contains(&depth) => Err(ContentError(format!("{}, surface {part}: depth must be 0..=24", at()))),
                _ => Ok(()),
            }
        };
        layers("ground", &surface.ground)?;
        layers("underwater", &surface.underwater)?;
        if let Some(shore) = &surface.shore {
            layers("shore", shore)?;
        }
        if let Some(cliff) = &surface.cliff {
            material("cliff", cliff)?;
        }
        for (i, rule) in surface.rules.iter().enumerate() {
            let part = format!("rule {}", i + 1);
            layers(&part, &rule.place)?;
            for condition in &rule.when {
                self.check_condition(condition, &|| format!("{}, surface {part}", at()))?;
            }
        }
        Ok(())
    }

    fn check_condition(&self, condition: &Condition, at: &dyn Fn() -> String) -> Result<(), ContentError> {
        let noise = |noise: &NoiseDef| {
            if noise.scale > 0.0 && (1..=6).contains(&noise.octaves) {
                Ok(())
            } else {
                Err(ContentError(format!("{}: noise `{}` needs a positive scale and 1..=6 octaves", at(), noise.name)))
            }
        };
        match condition {
            Condition::Underwater => Ok(()),
            Condition::Water(bodies) => {
                if bodies.is_empty() {
                    Err(ContentError(format!("{}: `Water` needs at least one kind of water", at())))
                } else {
                    Ok(())
                }
            }
            Condition::Deeper(a, b)
            | Condition::Slope(a, b)
            | Condition::Altitude(a, b)
            | Condition::Colder(a, b)
            | Condition::Warmer(a, b)
            | Condition::Wetter(a, b)
            | Condition::Drier(a, b)
            | Condition::Forest(a, b)
            | Condition::River(a, b) => {
                if a.is_finite() && b.is_finite() {
                    Ok(())
                } else {
                    Err(ContentError(format!("{}: thresholds must be finite", at())))
                }
            }
            Condition::Province(name) => {
                if self.provinces.iter().any(|p| p.matches(name)) {
                    Ok(())
                } else {
                    Err(ContentError(format!("{}: no rock province is named or tagged `{name}`", at())))
                }
            }
            Condition::Band(n, _) | Condition::Patch(n, _) => noise(n),
            Condition::Tapered(n, by, widths) => {
                noise(n)?;
                noise(by)?;
                if widths.is_empty() || widths.windows(2).any(|w| w[0].0 < w[1].0) {
                    Err(ContentError(format!("{}: tapered widths need descending thresholds", at())))
                } else {
                    Ok(())
                }
            }
            Condition::Chance(p) => {
                if (0.0..=1.0).contains(p) {
                    Ok(())
                } else {
                    Err(ContentError(format!("{}: a chance must be in 0..=1", at())))
                }
            }
            Condition::Not(inner) => self.check_condition(inner, at),
            Condition::Any(list) => list.iter().try_for_each(|c| self.check_condition(c, at)),
        }
    }

    /// Every cell of the band table must belong to some land biome that needs no aether
    /// (Terralith's exhaustive table), so the picker never falls back on a guess.
    fn check_coverage(&self) -> Result<(), ContentError> {
        let axes = [Axis::Temperature, Axis::Moisture, Axis::Continentalness, Axis::Erosion];
        let counts = axes.map(|axis| self.bands(axis).names.len());
        let index = |axis: Axis, band: &str| self.bands(axis).names.iter().position(|n| n == band).expect("checked");
        let mut covered = vec![false; counts.iter().product()];
        for biome in self.biomes.iter().filter(|b| b.layer == Layer::Surface && b.role == Role::Land) {
            for climate in biome.climate.iter().filter(|c| c.aether.is_none()) {
                let ranges = axes.map(|axis| match climate.bands(axis) {
                    Some((from, to)) => index(axis, from)..=index(axis, to),
                    None => 0..=self.bands(axis).names.len() - 1,
                });
                for t in ranges[0].clone() {
                    for m in ranges[1].clone() {
                        for c in ranges[2].clone() {
                            for e in ranges[3].clone() {
                                covered[((t * counts[1] + m) * counts[2] + c) * counts[3] + e] = true;
                            }
                        }
                    }
                }
            }
        }
        let holes: Vec<String> = (0..covered.len())
            .filter(|&i| !covered[i])
            .take(5)
            .map(|i| {
                let e = i % counts[3];
                let c = i / counts[3] % counts[2];
                let m = i / (counts[3] * counts[2]) % counts[1];
                let t = i / (counts[3] * counts[2] * counts[1]);
                let name = |axis: Axis, i: usize| self.bands(axis).names[i].as_str();
                format!(
                    "({}, {}, {}, {})",
                    name(Axis::Temperature, t),
                    name(Axis::Moisture, m),
                    name(Axis::Continentalness, c),
                    name(Axis::Erosion, e)
                )
            })
            .collect();
        if holes.is_empty() {
            Ok(())
        } else {
            let total = covered.iter().filter(|c| !**c).count();
            Err(ContentError(format!(
                "the biome table has {total} holes (temperature, moisture, continentalness, erosion), such as {}",
                holes.join(", ")
            )))
        }
    }
}

/// Checks blocks named in a definition; the first argument says where, for errors.
type BlockCheck<'a, T> = dyn Fn(&dyn Fn() -> String, &T) -> Result<(), ContentError> + 'a;

fn parse<T: DeserializeOwned>(at: &str, source: &str) -> Result<T, ContentError> {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_str(source)
        .map_err(|e| ContentError(format!("{at}: {e}")))
}

fn positive(at: &dyn Fn() -> String, what: &str, value: f32) -> Result<(), ContentError> {
    if value > 0.0 && value.is_finite() {
        Ok(())
    } else {
        Err(ContentError(format!("{}: {what} must be positive", at())))
    }
}

/// `stone` in package `base` is `base:stone`; qualified names stay as they are.
pub fn qualify(package: &str, name: &str) -> String {
    if name.contains(':') { name.to_owned() } else { format!("{package}:{name}") }
}

fn qualify_select(package: &str, select: &Select) -> Select {
    match select {
        Select::Name(name, weight) => Select::Name(qualify(package, name), *weight),
        Select::Tag(tag, weight) => Select::Tag(tag.clone(), *weight),
    }
}

fn qualify_material(package: &str, material: Material) -> Material {
    match material {
        Material::Block(name) => Material::Block(qualify(package, &name)),
        Material::Mix(selects) => Material::Mix(selects.iter().map(|s| qualify_select(package, s)).collect()),
        other => other,
    }
}

fn qualify_layers(package: &str, layers: LayersDef) -> LayersDef {
    LayersDef {
        top: qualify_material(package, layers.top),
        filler: layers.filler.map(|m| qualify_material(package, m)),
        depth: layers.depth,
    }
}


fn qualify_biome(package: &str, biome: RawBiome) -> RawBiome {
    let q = |name: &str| qualify(package, name);
    RawBiome {
        name: q(&biome.name),
        inherit: biome.inherit.as_deref().map(q),
        ecotone: biome.ecotone.map(|e| EcotoneDef { between: (q(&e.between.0), q(&e.between.1)), ..e }),
        surface: biome.surface.map(|s| SurfaceDef {
            rules: s
                .rules
                .into_iter()
                .map(|r| RuleDef {
                    when: r.when,
                    place: qualify_layers(package, r.place),
                })
                .collect(),
            ground: qualify_layers(package, s.ground),
            underwater: qualify_layers(package, s.underwater),
            shore: s.shore.map(|l| qualify_layers(package, l)),
            cliff: s.cliff.map(|m| qualify_material(package, m)),
        }),
        strata: biome.strata.map(|names| names.iter().map(|n| q(n)).collect()),
        stones: biome.stones.map(|s| match s {
            Stones::Province => Stones::Province,
            Stones::Blocks(selects) => Stones::Blocks(selects.iter().map(|s| qualify_select(package, s)).collect()),
        }),
        variants: biome.variants.map(|v| v.into_iter().map(|(name, share)| (q(&name), share)).collect()),
        ..biome
    }
}

/// Resolves `inherit`: a biome takes every field it leaves out from its parent, except its
/// name, variants and ecotone. Ecotones and variants take no climate unless they set their
/// own: they grow on a border or inside their parent, not beside it.
fn inherit(raw: &[(String, RawBiome)]) -> Result<Vec<BiomeDef>, ContentError> {
    let find = |name: &str| raw.iter().find(|(_, b)| b.name == name);
    let variants: Vec<&str> = raw.iter().flat_map(|(_, b)| b.variants.iter().flatten().map(|(v, _)| v.as_str())).collect();
    raw.iter()
        .map(|(at, biome)| {
            let at = || format!("{at}: biome `{}`", biome.name);
            // The chain of ancestors, nearest first.
            let mut chain = vec![biome];
            while let Some(parent) = &chain[chain.len() - 1].inherit {
                let Some((_, parent)) = find(parent) else {
                    return Err(ContentError(format!("{}: inherits from unknown biome `{parent}`", at())));
                };
                if chain.iter().any(|b| b.name == parent.name) {
                    return Err(ContentError(format!("{}: inherits from itself through `{}`", at(), parent.name)));
                }
                chain.push(parent);
            }
            let first = |field: &dyn Fn(&RawBiome) -> bool| chain.iter().find(|b| field(b)).copied();
            macro_rules! field {
                ($name:ident) => {
                    first(&|b| b.$name.is_some()).and_then(|b| b.$name.clone())
                };
            }
            let surface = field!(surface).ok_or_else(|| ContentError(format!("{}: needs a surface", at())))?;
            Ok(BiomeDef {
                name: biome.name.clone(),
                tags: field!(tags).unwrap_or_default(),
                layer: field!(layer).unwrap_or_default(),
                role: field!(role).unwrap_or_default(),
                climate: if biome.ecotone.is_some() || variants.contains(&biome.name.as_str()) {
                    biome.climate.clone().unwrap_or_default()
                } else {
                    field!(climate).unwrap_or_default()
                },
                ecotone: biome.ecotone.clone(),
                weight: field!(weight).unwrap_or(1.0),
                rarity: field!(rarity).unwrap_or_default(),
                surface,
                stones: field!(stones).unwrap_or_default(),
                strata: field!(strata).unwrap_or_default(),
                colors: field!(colors).unwrap_or_default(),
                variants: biome.variants.clone().unwrap_or_default(),
            })
        })
        .collect()
}

fn one() -> f32 {
    1.0
}

fn one_octave() -> i32 {
    1
}

fn temperature_axis() -> Axis {
    Axis::Temperature
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> BlockRegistry {
        BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap()
    }

    fn base_files() -> Vec<ContentFile> {
        BASE_FILES.iter().map(|(name, source)| ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() }).collect()
    }

    /// The base package with `biomes.ron` replaced by `source`, and an extra package.
    fn load_with(biomes: &str) -> Result<WorldContent, ContentError> {
        let mut files = base_files();
        files.iter_mut().find(|f| f.name == "biomes.ron").unwrap().source = biomes.into();
        WorldContent::from_files(files.iter(), &registry())
    }

    const GROUND: &str = "surface: (ground: (top: Block(\"grass_block\")), underwater: (top: Block(\"sand\")))";

    /// One land biome everywhere, plus `extra`.
    fn biomes(extra: &str) -> String {
        format!("#![enable(implicit_some)]\n(biomes: [(name: \"everywhere\", climate: [()], {GROUND}), {extra}])")
    }

    #[test]
    fn base_content_loads() {
        let content = WorldContent::base(&registry()).unwrap();
        assert!(content.biomes.len() >= 10, "{} biomes", content.biomes.len());
        assert!(content.biomes.iter().all(|b| b.name.starts_with("base:")));
        assert!(content.provinces.len() >= 6);
    }

    #[test]
    fn inheritance_fills_in_fields() {
        let content = load_with(&biomes(&format!(
            "(name: \"parent\", tags: [\"a\"], climate: [(temperature: (\"cold\", \"cool\"))], rarity: Rare, {GROUND}),
             (name: \"child\", inherit: \"parent\", weight: 2.0)"
        )))
        .unwrap();
        let (parent, child) = (content.biome("base:parent").unwrap(), content.biome("base:child").unwrap());
        assert_eq!((child.tags.clone(), child.climate.clone(), child.rarity, child.weight), (parent.tags.clone(), parent.climate.clone(), Rarity::Rare, 2.0));
    }

    #[test]
    fn broken_content_fails_with_clear_errors() {
        let error = |extra: &str| load_with(&biomes(extra)).unwrap_err().0;
        let cases = [
            // Unknown fields are refused.
            ("(name: \"a\", climate: [()], colour: (grass: (1, 2, 3)), SURFACE)", "colour"),
            ("(name: \"a\", climate: [()], surface: (ground: (top: Block(\"grass_blok\")), underwater: (top: Block(\"sand\"))))", "unknown block `base:grass_blok`"),
            ("(name: \"a\", climate: [(temperature: (\"warm\", \"cold\"))], SURFACE)", "is empty"),
            ("(name: \"a\", climate: [(temperature: (\"balmy\", \"warm\"))], SURFACE)", "unknown Temperature band `balmy`"),
            ("(name: \"a\", climate: [(aether: (0.9, 0.2))], SURFACE)", "aether range"),
            ("(name: \"a\", SURFACE)", "never grows"),
            ("(name: \"a\", climate: [()], variants: [(\"nowhere\", 0.2)], SURFACE)", "unknown variant `base:nowhere`"),
            ("(name: \"a\", inherit: \"b\"), (name: \"b\", inherit: \"a\")", "inherits from itself"),
            ("(name: \"a\", climate: [()], weight: 0.0, SURFACE)", "weight must be positive"),
            ("(name: \"a\", climate: [()], surface: (rules: [(when: [Province(\"chalk\")], place: (top: Stones))], ground: (top: Stones), underwater: (top: Stones)))", "rock province"),
            ("(name: \"a\", climate: [()], surface: (ground: (top: Mix([Tag(\"no_such_tag\", 1.0)])), underwater: (top: Stones)))", "tag `no_such_tag`"),
        ];
        for (extra, expected) in cases {
            let message = error(&extra.replace("SURFACE", GROUND));
            assert!(message.contains(expected), "expected `{expected}` in: {message}");
            assert!(message.contains("biomes.ron") || expected == "inherits from itself", "no file in: {message}");
        }
    }

    #[test]
    fn holes_in_the_biome_table_are_refused() {
        let only_cold = format!("#![enable(implicit_some)]\n(biomes: [(name: \"cold\", climate: [(temperature: (\"frozen\", \"cold\"))], {GROUND})])");
        let message = load_with(&only_cold).unwrap_err().0;
        assert!(message.contains("holes") && message.contains("cool"), "{message}");
    }
}
