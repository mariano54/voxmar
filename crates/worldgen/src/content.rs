//! World content: the generator's definitions, read from content packages the way blocks
//! are, so a mod adds a biome, a tree or a rock province exactly as the base game does.
//!
//! Each package may hold `climate.ron` (the bands every biome is placed by, and the blocks
//! of snow and ice), `rocks.ron` (rock provinces), `biomes.ron`, `cave_biomes.ron` (biomes
//! of the `Underground` layer, in a file of their own), `surfaces.ron` (surface rule sets
//! biomes share), `trees.ron`, `plants.ron`, `decorations.ron` (rocks, logs, bushes,
//! patches and tree ornaments), `land.ron` (the land's noise fields and terrain splines) and
//! `terrain.ron` (terrain shapes). Names are qualified with
//! the package (`base:taiga`). Loading resolves `inherit` and shared rule sets, then checks
//! every reference: blocks, provinces, biomes, trees, plants, band names, and that the
//! biome table has no holes. A broken package fails at load with a message naming the
//! file, the definition and what is wrong, never later in generation.
//!
//! Trees, plants and stones are a shared pool. Each tree and plant says what climate it
//! likes, what ground it takes root in and where it grows (by a stream, in shade), and
//! carries tags; biomes select from the pool by tag or name with a weight, and generation
//! multiplies that weight by how well the climate at each spot suits the candidate. So a
//! new flower with the right tags grows in every biome that fits, and a biome can still
//! feature a species of its own by naming it.

use std::fmt;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use voxmar_engine::block::{BlockRegistry, RenderKind};
use voxmar_engine::content::{Content, ContentFile};

use crate::levels::{Homes, LevelsDef, MAX_LEVEL};
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

/// Single-valued data (one climate, one land, one set of trail materials) that a later
/// package sets field by field (Design draft 91): its file names only what it changes,
/// which is laid over what the packages before it set, and loading notes which package
/// replaced which fields. So a mod tunes one number without restating the file.
/// [`layered!`] declares one. The engine's `settable!` (crates/engine/src/content.rs) is
/// the same rule for creature limits: two mechanisms for one rule, until `settable!` grows
/// nesting and named lists and this one is built on it.
trait Layered: Sized {
    /// Every field as an option: what a later package's file sets.
    type Set: DeserializeOwned;
    /// These values as a set of every field, to qualify them as a later package's.
    fn into_set(self) -> Self::Set;
    /// Lays `set` over these values, naming each field it set (`scales.aether`, a named
    /// list's entry as `noises.erosion`) after `path`.
    fn lay(&mut self, set: Self::Set, path: &str, changed: &mut Vec<String>);
}

/// Declares `$set` for a [`Layered`] `$whole`: its fields as options, as a later package's
/// file reads them. `replace` fields are replaced whole; `nested` ones are laid field by
/// field through their own set; `named` lists of `(name, value)` replace the entries a
/// package names, in place, and add the ones it adds at the end.
macro_rules! layered {
    ($set:ident for $whole:ident {
        replace { $($field:ident: $ty:ty),* $(,)? }
        $(nested { $($nfield:ident: $nset:ty),* $(,)? })?
        $(named { $($lfield:ident: $lty:ty),* $(,)? })?
    }) => {
        #[doc = concat!("[`", stringify!($whole), "`] as a later package sets it: only the fields it changes.")]
        #[derive(Clone, Debug, Default, Deserialize)]
        #[serde(deny_unknown_fields, default)]
        pub struct $set {
            $(pub $field: Option<$ty>,)*
            $($(pub $nfield: Option<$nset>,)*)?
            $($(pub $lfield: Option<Vec<(String, $lty)>>,)*)?
        }

        impl Layered for $whole {
            type Set = $set;

            fn into_set(self) -> $set {
                // Exhaustive (no `..`): a field added to `$whole` and not listed above is a
                // compile error here, not a field a later package silently cannot set. A
                // compile error here means a field is missing from the `layered!` list; never
                // add `..`.
                let $whole { $($field,)* $($($nfield,)*)? $($($lfield,)*)? } = self;
                $set {
                    $($field: Some($field),)*
                    $($($nfield: Some($nfield.into_set()),)*)?
                    $($($lfield: Some($lfield),)*)?
                }
            }

            fn lay(&mut self, set: $set, path: &str, changed: &mut Vec<String>) {
                $(
                    if let Some(value) = set.$field {
                        self.$field = value;
                        changed.push(format!("{path}{}", stringify!($field)));
                    }
                )*
                $($(
                    if let Some(nested) = set.$nfield {
                        self.$nfield.lay(nested, &format!("{path}{}.", stringify!($nfield)), changed);
                    }
                )*)?
                $($(
                    for (name, value) in set.$lfield.into_iter().flatten() {
                        changed.push(format!("{path}{}.{name}", stringify!($lfield)));
                        match self.$lfield.iter_mut().find(|(n, _)| *n == name) {
                            Some(entry) => entry.1 = value,
                            None => self.$lfield.push((name, value)),
                        }
                    }
                )*)?
            }
        }
    };
}

/// The package that last set each field of a [`Layered`] value, by field path.
type Setters = std::collections::HashMap<String, String>;

/// One package's single-valued data, laid over the packages' before it (see [`Layered`]):
/// the first file read whole (every field it must have), later ones field by field, each
/// noted. `setters` holds the package that last set each field, so a note names the
/// package whose value a later one replaces, field by field. `qualify` names the set's
/// blocks in the package's namespace.
fn lay_file<T: Layered + DeserializeOwned + Clone>(
    value: &mut Option<(T, Setters)>,
    file: &ContentFile,
    qualify: impl Fn(&mut T::Set),
    notes: &mut Vec<String>,
) -> Result<(), ContentError> {
    let at = format!("{}/{}", file.package, file.name);
    match value {
        None => {
            let mut whole = parse::<T>(&at, &file.source)?;
            let mut set = whole.clone().into_set();
            qualify(&mut set);
            let mut changed = Vec::new();
            whole.lay(set, "", &mut changed);
            *value = Some((whole, changed.into_iter().map(|path| (path, file.package.clone())).collect()));
        }
        Some((whole, setters)) => {
            let mut set = parse::<T::Set>(&at, &file.source)?;
            qualify(&mut set);
            lay_set(whole, setters, set, file, "", notes);
        }
    }
    Ok(())
}

/// Lays a later package's settings over a value, retaining the setter of every field.
/// `path` names a section within a file, such as `tarns.` in terrain.ron.
fn lay_set<T: Layered>(whole: &mut T, setters: &mut Setters, set: T::Set, file: &ContentFile, path: &str, notes: &mut Vec<String>) {
    let at = format!("{}/{}", file.package, file.name);
    let mut changed = Vec::new();
    whole.lay(set, path, &mut changed);
    // One note for each package whose fields were replaced (in the order they were
    // first set), and one for the fields that are new.
    let mut by_package: Vec<(String, Vec<String>)> = Vec::new();
    let mut added = Vec::new();
    for path in changed {
        match setters.insert(path.clone(), file.package.clone()) {
            Some(earlier) => match by_package.iter_mut().find(|(p, _)| *p == earlier) {
                Some((_, paths)) => paths.push(path),
                None => by_package.push((earlier, vec![path])),
            },
            None => added.push(path),
        }
    }
    for (earlier, paths) in by_package {
        notes.push(format!("{at} replaces the {} {earlier} set ({})", file.name, paths.join(", ")));
    }
    if !added.is_empty() {
        notes.push(format!("{at} adds to the {} ({})", file.name, added.join(", ")));
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
    /// How far the regional climate's own fields vary, in blocks.
    #[serde(default)]
    pub scales: ScalesDef,
    /// The typical distance from the equator to a pole, in blocks: how wide the climate
    /// zones are. Each world's own lies within a tenth of it.
    #[serde(default = "default_zones")]
    pub zones: f32,
    /// What the humidity biomes are placed by is made of.
    #[serde(default)]
    pub humidity: HumidityDef,
    pub frost: FrostDef,
    /// How biome levels scale finds and how distance from spawn leans the biome picker
    /// towards higher levels (see `crate::levels`).
    #[serde(default)]
    pub levels: LevelsDef,
}

layered!(ClimateSet for ClimateDef {
    replace { temperature: BandsDef, moisture: BandsDef, continentalness: BandsDef, erosion: BandsDef, remaps: Vec<RemapDef>, zones: f32 }
    nested { scales: ScalesSet, humidity: HumiditySet, frost: FrostSet, levels: LevelsSet }
});
layered!(LevelsSet for LevelsDef {
    replace { step: f32, earlier: f32, lean: f32, reach: f32, wildness: f32, trees: Vec<String> }
    nested { homes: HomesSet }
});
layered!(HomesSet for Homes { replace { common: u8, uncommon: u8, rare: u8, very_rare: u8, legendary: u8 } });
layered!(ScalesSet for ScalesDef { replace { anomaly: f32, wetness: f32, weirdness: f32, aether: f32, fertility: f32, variant: f32 } });
layered!(HumiditySet for HumidityDef { replace { median: f32, spread: f32, wetness: f32, wind: f32, belts: f32 } });
layered!(FrostSet for FrostDef {
    replace { snow: String, powder: String, snowy_ground: String, ice: String, thick_ice: String, rare_ice: String, blanket: Vec<BlanketDef> }
});

/// Wavelengths of the regional climate's own noise fields, in blocks: how large the
/// regions they shape are.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScalesDef {
    /// Warm and cold anomalies of the sea-level temperature (±5 °C).
    pub anomaly: f32,
    /// Wetter and drier regions.
    pub wetness: f32,
    /// The field rare biomes take the extremes of.
    pub weirdness: f32,
    /// Where the land is fantastical.
    pub aether: f32,
    /// How rich the soil is.
    pub fertility: f32,
    /// The field that picks a biome's variants.
    pub variant: f32,
}

/// The humidity biomes, plants and the land's regions read, in moisture's units
/// (precipitation over evaporation): a slow wetness field, the rain the prevailing wind
/// brings (wet coasts, dry interiors and rain shadows) and the latitude's wet and dry belts,
/// weighed together. It leaves out how much the local warmth evaporates, which varies with
/// latitude as fast as temperature does: biomes split by both would come in thin bands.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HumidityDef {
    /// The median humidity of a world of average humidity.
    pub median: f32,
    /// How widely it spreads: the standard deviation of its logarithm.
    pub spread: f32,
    /// The weights of the slow wetness field, of the wind's rain and of the belts.
    pub wetness: f32,
    pub wind: f32,
    pub belts: f32,
}

impl Default for HumidityDef {
    fn default() -> Self {
        Self { median: 0.72, spread: 0.6, wetness: 0.8, wind: 0.5, belts: 0.33 }
    }
}

fn default_zones() -> f32 {
    10000.0
}

impl Default for ScalesDef {
    fn default() -> Self {
        Self { anomaly: 8000.0, wetness: 6000.0, weirdness: 11000.0, aether: 2200.0, fertility: 700.0, variant: 800.0 }
    }
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
    /// The snow blanket: where snow lies on the ground, blocks above it (in tree crowns,
    /// on rocks) that match one of these and are open to the sky turn into the block
    /// given, so snow covers every level down through the canopy.
    #[serde(default)]
    pub blanket: Vec<BlanketDef>,
}

/// One rule of the snow blanket: blocks `covers` matches, unless they carry a tag in
/// `not`, turn into `to` where snow lies on them. Built blocks (`not: ["building"]`)
/// keep their own look and their drops; snow stops on them.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlanketDef {
    pub covers: BlockMatch,
    pub to: String,
    #[serde(default)]
    pub not: Vec<String>,
}

impl BlanketDef {
    /// Whether the rule turns `def` to snow.
    pub fn applies(&self, def: &voxmar_engine::block::BlockDef) -> bool {
        let matched = match &self.covers {
            BlockMatch::Name(name) => def.name == *name,
            BlockMatch::Tag(tag) => def.has_tag(tag),
        };
        matched && !self.not.iter().any(|tag| def.has_tag(tag))
    }
}

/// One block by name, or every block carrying a tag.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum BlockMatch {
    Name(String),
    Tag(String),
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
    /// Caves: chosen in 3D, by depth, height and rock province as well as climate. Their
    /// `surface` lines cave floors (`ground`, or `underwater` under water) and ceilings
    /// (`cliff`); their `stones` (or `strata`) are the rock around them; and their
    /// `cave_decoration` grows from floors and ceilings. See `crate::underground`.
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
    /// Underground biomes only: blocks below the surface.
    pub depth: Option<(f32, f32)>,
    /// Underground biomes only: height above sea level (negative below it).
    pub height: Option<(f32, f32)>,
    /// Underground biomes only: the rock provinces it lies in, by name or tag.
    pub provinces: Option<Vec<String>>,
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
    /// Deeper into a clearing than this: 0 outside, rising across its soft edge to 1.
    Clearing(f32, f32),
    /// The forest's structure above this: 0 in its gaps and scrubby edges ..= 1 in its
    /// old-growth cores (see `Site::stand`).
    Stand(f32, f32),
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
    /// Shared rule sets from `surfaces.ron` (slopes, trails), tried in this order before
    /// the biome's own rules.
    #[serde(default)]
    pub include: Vec<String>,
    pub ground: LayersDef,
    pub underwater: LayersDef,
    #[serde(default)]
    pub shore: Option<LayersDef>,
    /// The stones beneath when left out.
    #[serde(default)]
    pub cliff: Option<Material>,
}

/// Which way a cave decoration grows.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Side {
    /// Up from a cave floor.
    Floor,
    /// Down from a cave ceiling.
    Ceiling,
}

/// Something growing in a cave: a plant or mushroom on the floor, a stalagmite, a
/// stalactite, an icicle, hanging roots or vines, a crystal. It grows straight up or down
/// within its block column, into open air only.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecorationDef {
    pub on: Side,
    /// The chance per block of floor or ceiling. A position takes the first decoration of
    /// its side that it rolls, in order.
    pub chance: f32,
    /// Further conditions on the column, as in surface rules (patches, warmth).
    #[serde(default)]
    pub when: Vec<Condition>,
    /// Only on floor or ceiling blocks carrying one of these tags (soil for plants).
    #[serde(default)]
    pub on_tags: Vec<String>,
    pub block: Material,
    /// How many blocks it grows, from the first number to the second; one when left out.
    #[serde(default = "single")]
    pub length: (i32, i32),
    /// A different last block, with this chance: glowing tips.
    #[serde(default)]
    pub tip: Option<(Material, f32)>,
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
    level: Option<u8>,
    surface: Option<SurfaceDef>,
    stones: Option<Stones>,
    field_stones: Option<Vec<Select>>,
    strata: Option<Vec<String>>,
    colors: Option<Colors>,
    flora: Option<FloraDef>,
    atmosphere: Option<AtmosphereDef>,
    variants: Option<Vec<(String, f32)>>,
    cave_decoration: Option<Vec<DecorationDef>>,
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
    /// How harsh, rare and aether-touched it is, 1 ..= 10 (`crate::levels`): it makes its
    /// rarer finds commoner and, far from spawn, the biome itself likelier. 1 when left
    /// out (and not inherited).
    pub level: u8,
    pub surface: SurfaceDef,
    pub stones: Stones,
    /// The stones lying on gentle open ground (boulders, rocks), where a biome wants its own
    /// rather than the bare rock's: a bright meadow's pale or mossy stone, not dark basalt.
    /// Empty for the rock's own. Cliffs, slopes, scree and streams keep the rock's.
    pub field_stones: Vec<Select>,
    /// Blocks of banded rock, one block each, bottom to top, repeating up the world.
    pub strata: Vec<String>,
    pub colors: Colors,
    pub flora: FloraDef,
    pub atmosphere: AtmosphereDef,
    /// Sub-biomes and the share of this biome's land each takes.
    pub variants: Vec<(String, f32)>,
    /// What grows from the floors and ceilings of an underground biome's caves.
    pub cave_decoration: Vec<DecorationDef>,
}

// ---------------------------------------------------------------------------------------
// trees.ron and plants.ron

/// What a surface offers plants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Ground {
    /// Grass, dirt and other soils.
    Soil,
    /// Snow-covered soil.
    Snow,
    Sand,
    /// The floor of shallow water.
    Shallows,
    /// Rock, gravel, ice: nothing grows.
    Bare,
}

/// Where in a landscape something grows, beyond climate and ground.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Habitat {
    /// Wherever its climate and ground suit it.
    #[default]
    Anywhere,
    /// In the open: thins out under trees.
    Open,
    /// Under trees.
    Shade,
    /// Thickest under trees, sparser in the open (ferns).
    Understory,
    /// Along rivers and lake shores, just above the water.
    Water,
    /// On beaches just above the sea.
    Shore,
    /// At the edge of warm shallows, rooted under water.
    Shallows,
    /// On the banks of still lakes (not rivers or creeks), just above the water: mountain
    /// tarns for the sky lotus.
    LakeShore,
}

impl Habitat {
    /// Whether it runs along the edge of water, a thin line through the land: trees of
    /// such a habitat grow beside a forest rather than in it, and a rare herb's patches
    /// start from it where it crosses their cell rather than at a random spot.
    pub fn edge(self) -> bool {
        matches!(self, Self::Water | Self::Shore | Self::Shallows | Self::LakeShore)
    }
}

/// The climate something grows best in: inside both ranges it thrives, and it fades out
/// over a margin beyond them (2 °C, 0.12 of moisture). An axis left out suits everywhere.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Comfort {
    /// °C.
    pub temperature: Option<(f32, f32)>,
    /// Soil moisture: precipitation over evaporation, with ground water near rivers.
    pub moisture: Option<(f32, f32)>,
}

/// The shape of the leafy part of a tree. Sizes are those of a mature tree; young trees
/// and giants scale them.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum Crown {
    /// A broadleaf dome grown by space colonisation (see `growth`): limbs leave the trunk
    /// from `start` (a fraction of it, a half when left out) and branch out to fill a crown
    /// as wide as they `reach` plus the billows of leaves at their ends, which are `radius`
    /// across; `squash` flattens the crown and its billows.
    Round {
        reach: (f32, f32),
        radius: (f32, f32),
        squash: f32,
        #[serde(default = "half")]
        start: f32,
    },
    /// A cone of drooping limbs carrying needles, from `start` (a fraction of the trunk) to
    /// a spike over its top, `radius` across at its foot.
    Conical { radius: (f32, f32), start: f32 },
    /// A few billowed pads of needles on upswept limbs high on a bare trunk, from `start`
    /// (a fraction of it) up; `radius` is how far the pads reach from the trunk.
    Pine { radius: (f32, f32), start: f32 },
    /// A narrow flame of leaves from `start` (a fraction of the trunk) to well above its
    /// top, widest a third of the way up (cypresses).
    Spire { radius: (f32, f32), start: f32 },
    /// A domed cap with a hanging rim, on a stem (giant mushrooms).
    Cap { radius: (f32, f32) },
    /// A wide, flat crown of billows on limbs that fork off the trunk from `fork` (a
    /// fraction of it, 0.45 when left out) and spread as far as they `reach` plus the
    /// billows' `radius`. `squash` is how thick the crown is against its width (0.4 when
    /// left out).
    Umbrella {
        reach: (f32, f32),
        radius: (f32, f32),
        #[serde(default = "pads")]
        squash: f32,
        #[serde(default = "fork")]
        fork: f32,
    },
    /// A round crown on short limbs with curtains of leaves `drape` blocks long hanging
    /// from its underside.
    Weeping { radius: (f32, f32), drape: (f32, f32) },
    /// Long drooping fronds at the top of a bare trunk.
    Palm { fronds: (i32, i32), length: (f32, f32) },
    /// A low, lumpy mound of leaves on a stub of wood, draped over the ground.
    Bush { radius: (f32, f32) },
    /// Bare limbs stepping up and out from a snag, a share `fork` of them splitting into
    /// two twigs (a half when left out). A share `broken` of snags (a half when left out)
    /// stand broken off, the rest of the trunk lying beside them.
    Dead {
        branches: (i32, i32),
        reach: (f32, f32),
        #[serde(default = "half")]
        broken: f32,
        #[serde(default = "half")]
        fork: f32,
    },
    /// No crown at all (cacti).
    Bare,
}

fn half() -> f32 {
    0.5
}

fn pads() -> f32 {
    0.4
}

fn fork() -> f32 {
    0.45
}

impl Crown {
    /// Farthest the crown reaches from the top of the trunk, horizontally, on a mature
    /// tree.
    pub fn reach(&self) -> f32 {
        match *self {
            Crown::Round { reach, radius, .. } | Crown::Umbrella { reach, radius, .. } => reach.1 + radius.1,
            Crown::Conical { radius, .. } | Crown::Spire { radius, .. } | Crown::Bush { radius } => radius.1,
            Crown::Pine { radius, .. } => radius.1 + 2.0,
            Crown::Weeping { radius, .. } => radius.1 + 2.5,
            Crown::Cap { radius } => radius.1 + 0.5,
            Crown::Palm { length, .. } => length.1,
            // Twigs reach past the limbs' elbows.
            Crown::Dead { reach, .. } => reach.1 * 1.3,
            Crown::Bare => 0.0,
        }
    }

    /// Whether it casts a canopy (shrubs, bare branches and cacti do not).
    pub fn shades(&self) -> bool {
        !matches!(self, Crown::Bush { .. } | Crown::Dead { .. } | Crown::Bare)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Roots {
    #[default]
    None,
    /// Fins of wood flaring out from the base of a giant trunk.
    Buttress,
    /// Arching roots that hold the trunk above water.
    Stilts,
}

/// Where each wood comes from (`woods` in trees.ron), so a region's timber is its own
/// trees': a wood named after a tree builds a tree of another family only where a tree of
/// its own family grows too, which the base content's tests hold it to. The common woods
/// grow almost everywhere and may build any tree anywhere.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct WoodsDef {
    /// Woods (log blocks) that build any tree anywhere.
    pub common: Vec<String>,
    /// Every other wood a tree is built of, and the trees of its family.
    pub families: std::collections::BTreeMap<String, Vec<String>>,
}

impl WoodsDef {
    /// Adds package `package`'s woods (names qualified) from the file at `at`: family
    /// members join a family's, and a wood the package classes the other way (common, or
    /// with a family) is classed its way instead, with a note. A package may not class one
    /// wood both ways.
    fn add(&mut self, at: &str, package: &str, other: &WoodsDef, notes: &mut Vec<String>) -> Result<(), ContentError> {
        if let Some(wood) = other.common.iter().find(|w| other.families.contains_key(*w)) {
            return Err(ContentError(format!("{at}: `{wood}` is listed both as a common wood and with a family")));
        }
        for wood in &other.common {
            if let Some(family) = self.families.remove(wood) {
                notes.push(format!("{package}/trees.ron makes {wood} a common wood, replacing its family {family:?}"));
            }
            if !self.common.contains(wood) {
                self.common.push(wood.clone());
            }
        }
        for (wood, family) in &other.families {
            if let Some(i) = self.common.iter().position(|w| w == wood) {
                self.common.remove(i);
                notes.push(format!("{package}/trees.ron gives {wood} a family, replacing it as a common wood"));
            }
            let members = self.families.entry(wood.clone()).or_default();
            for tree in family {
                if !members.contains(tree) {
                    members.push(tree.clone());
                }
            }
        }
        Ok(())
    }
}

/// A kind of tree: a handful of shape parameters, its blocks, and where it grows.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TreeDef {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub rarity: Rarity,
    pub log: String,
    /// Leaves; a tree without them has a bare crown or none.
    #[serde(default)]
    pub leaves: Option<String>,
    /// Leaves used instead on this share of trees (flowering azaleas).
    #[serde(default)]
    pub alternate: Option<(String, f32)>,
    /// The log when left out.
    #[serde(default)]
    pub root_block: Option<String>,
    /// A fruiting kind of its leaves, for this share of the crown's outer leaves.
    #[serde(default)]
    pub fruit: Option<Fruit>,
    /// Trunk height, blocks.
    pub height: (i32, i32),
    /// A 2×2 trunk.
    #[serde(default)]
    pub thick: bool,
    /// How far the top of the trunk drifts sideways, in blocks.
    #[serde(default)]
    pub lean: f32,
    #[serde(default)]
    pub roots: Roots,
    pub crown: Crown,
    /// The ground it takes root in; soil when left out.
    #[serde(default = "soil")]
    pub ground: Vec<Ground>,
    #[serde(default)]
    pub habitat: Habitat,
    #[serde(default)]
    pub comfort: Comfort,
    /// Where in the forest's structure it grows (`Site::stand`, 0 in gaps and scrubby
    /// edges ..= 1 in old-growth cores), fading out over a small margin: giants in the
    /// cores, shrubs at the edges. Everywhere when left out.
    #[serde(default)]
    pub stand: Option<(f32, f32)>,
}

/// Fruit on a tree: a leaf block bearing it (`leaves`, rendered as leaves, whose regrowth
/// the game handles), for a `share` of the crown's outer leaves, most of them on its
/// underside and rim, where fruit hangs and can be reached.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Fruit {
    pub leaves: String,
    pub share: f32,
}

/// What a plant is good for, read by cooking, alchemy and dyeing (later). Left out, each
/// comes from the plant block's tags (`edible`, `alchemy:healing`, `dye:red`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PlantProperties {
    /// Food value of one plant, 0 for none.
    pub nutrition: Option<f32>,
    /// Alchemical properties.
    pub alchemy: Option<Vec<String>>,
    /// The dye it makes.
    pub dye: Option<String>,
}

/// How a plant spreads, which decides the pattern it grows in.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Spread {
    /// Carpets the ground (grasses, ferns), in lusher and sparser patches.
    #[default]
    Cover,
    /// In drifts of one kind (flowers).
    Drifts,
    /// Here and there on its own (bushes, mushrooms, reeds).
    Alone,
}

/// A small plant.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlantDef {
    pub name: String,
    /// The plant block; its looks (variants) are chosen by position.
    pub block: String,
    /// The upper half of a two-block plant.
    #[serde(default)]
    pub top: Option<String>,
    /// Grows as a stack of `block` this many blocks tall (reeds, cane).
    #[serde(default)]
    pub stack: Option<(i32, i32)>,
    /// Added to the block's own tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The block's rarity when left out.
    #[serde(default)]
    pub rarity: Option<Rarity>,
    #[serde(default = "soil")]
    pub ground: Vec<Ground>,
    #[serde(default)]
    pub habitat: Habitat,
    #[serde(default)]
    pub spread: Spread,
    #[serde(default)]
    pub comfort: Comfort,
    #[serde(default)]
    pub properties: PlantProperties,
    /// Its look where snow lies: it grows on snowy ground as this block. Plants without one
    /// do not grow under snow.
    #[serde(default)]
    pub snowy: Option<String>,
    /// Grows only in the biomes that name it (in their plants or wonders), never through a
    /// tag, the forage sweep, a decoration's selector or the wonders every biome takes by
    /// default: a rare herb that keeps to its one or two biomes.
    #[serde(default)]
    pub named_only: bool,
    /// How it grows in the biomes that name it: in small patches, at a rate of its own
    /// (see `Patches`), rather than by the biome's weight among its other plants.
    #[serde(default)]
    pub patches: Option<Patches>,
}

/// A rare herb's patches: about `rate` plants per million blocks where it can root, times
/// the weight a biome names it with, in patches of `size` plants (1 to 25). `aim` is the
/// rate design wants it to reach per million blocks of each biome naming it, which the
/// census test holds it to.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Patches {
    pub rate: f32,
    pub size: (u32, u32),
    pub aim: f32,
}

/// The most plants a patch may have.
pub const PATCH_MOST: u32 = 25;

/// What a biome grows, drawn from the shared pools of trees and plants.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct FloraDef {
    /// Tree cover where the forest is densest, 0 ..= 1.
    pub forest: f32,
    /// Trees, by selector. Trees of a particular habitat (by water, on shores) grow there
    /// beside the forest, at their weight times a fixed chance.
    pub trees: Vec<Select>,
    /// How much of the ground carpeting plants cover, 0 ..= 1.
    pub cover: f32,
    /// How much of the open ground flowers take, in drifts, 0 ..= 1.
    pub flowers: f32,
    /// How far the land is in bloom beyond the drifts every biome has, 0 ..= 1: 0 for
    /// most biomes, where flowers are sparse; high in the rare flower meadows and
    /// lavender fields, which bloom throughout.
    pub bloom: f32,
    /// Plants, by selector: shares among the carpeting plants and among the flowers; for
    /// plants that grow alone, the weight is their chance in a hundred per block.
    pub plants: Vec<Select>,
    /// Rare fantastical trees and plants, by selector, more common where aether is high:
    /// plants scattered, trees in rare lone trees and tiny groves. Everything tagged
    /// `wonder` that suits the climate when left out; trees named in `trees` are part of
    /// the forest instead.
    pub wonders: Option<Vec<Select>>,
    /// Decorations from `decorations.ron`, by selector; the weight scales each one's
    /// density. When left out, every decoration carrying one of the biome's tags.
    pub decorations: Option<Vec<Select>>,
    /// Tags whose trees, plants and decorations never grow here, whatever selects them.
    pub exclude: Vec<String>,
    /// The colours the biome keeps to (see `crate::hue`).
    pub palette: PaletteDef,
}

/// A biome's palette: anchor colours its vegetation is weighed against, so kinds whose
/// colours sit together grow freely and a clashing one is a rare accent.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PaletteDef {
    /// The canopy's colour; the biome's foliage colour when left out, so tinted leaves of
    /// every kind fit and strongly coloured ones (red maple, pink cherry) stand out.
    pub canopy: Option<(u8, u8, u8)>,
    /// The flowers' colour; when left out, any, each place growing a few kinds that are
    /// neighbours in hue.
    pub flowers: Option<(u8, u8, u8)>,
}

// ---------------------------------------------------------------------------------------
// decorations.ron

/// A field a decoration's density follows.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum Field {
    /// Tree cover, 0 ..= 1.
    Forest,
    /// The forest's structure, 0 in gaps and scrubby edges ..= 1 in old-growth cores.
    Stand,
    /// 0 outside clearings, rising across their soft edge to 1.
    Clearing,
    /// How rich the soil is, 0 ..= 1.
    Fertility,
    /// Soil moisture.
    Moisture,
    /// °C.
    Temperature,
    /// Steepest step to a neighbour, in blocks.
    Slope,
    /// Nearness to a river or lake, 0 ..= 1.
    River,
    /// Blocks above the sea.
    Altitude,
    /// How fantastical the land is, 0 ..= 1.
    Aether,
    /// A named noise field, −1 ..= 1 (shared with surface rules of the same name).
    Noise(NoiseDef),
}

/// A density ramp: `(field, from, to)` scales the density by 0 where the field is at
/// `from` rising smoothly to 1 at `to`; with `from` above `to` it falls instead.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Follow(pub Field, pub f32, pub f32);

/// What a decoration is. Its shape decides the stage it is placed in, in this fixed
/// order: patches of ground, rocks, trees (bushes), tree ornaments, logs; then the plant
/// stages (flowers, fungi, grass) and the snow blanket.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum Shape {
    /// The ground repainted in a ragged disc: dirt, gravel or moss patches, worn ground.
    Patch { radius: (f32, f32), top: Material },
    /// A lump of rock half sunk into the ground; `moss` is the chance, in damp shade, of
    /// moss on each block of its top.
    Boulder { radius: (f32, f32), stones: Material, #[serde(default)] moss: f32 },
    /// A few small stones scattered within `spread` blocks.
    Rocks { count: (i32, i32), spread: f32, size: (f32, f32), stones: Material, #[serde(default)] moss: f32 },
    /// A mound of leaves on a stub of wood: of the trees growing there, or of `leaves`
    /// (and `log`) where given.
    Bush { radius: (f32, f32), #[serde(default)] leaves: Option<String>, #[serde(default)] log: Option<String> },
    /// A fallen trunk of the trees growing there, lying only where the ground holds it:
    /// `thick` is the chance of a big trunk, `hollow` of a big one being hollow, `branches`
    /// of side branches, `stump` of the stump it broke from, and `growth` of a plant from
    /// `plants` on each block of its top.
    FallenLog {
        length: (i32, i32),
        #[serde(default)]
        thick: f32,
        #[serde(default)]
        hollow: f32,
        #[serde(default)]
        branches: f32,
        #[serde(default)]
        stump: f32,
        #[serde(default)]
        growth: f32,
        #[serde(default)]
        plants: Vec<Select>,
    },
    /// What is left of a tree that broke: a stump `height` blocks tall, with a chance
    /// `growth` of a plant from `plants` on top.
    Stump {
        height: (i32, i32),
        #[serde(default)]
        growth: f32,
        #[serde(default)]
        plants: Vec<Select>,
    },
    /// A ring of standing stones `radius` blocks across, `count` of them, each a pillar
    /// `height` blocks tall sunk into the ground: an old landmark.
    StandingStones { radius: (f32, f32), count: (i32, i32), height: (i32, i32), stones: Material },
    /// A ring of plants (mushrooms) `radius` blocks across, one block wide.
    FairyRing { radius: (f32, f32), plants: Vec<Select> },
    /// `count` of `block` lying loose on the ground within `spread` blocks: things a hand
    /// picks up (flint nodules).
    Scatter { count: (i32, i32), spread: f32, block: String },
    /// Ornament: the ground under the tree's crown, where `cover` of it is reached,
    /// becomes `top` (needles, leaf mould), densest near the trunk.
    Litter { top: Material, cover: f32 },
    /// Ornament: heaps of the tree's own leaves on the ground under its crown.
    LeafPiles { cover: f32 },
    /// Ornament: plants from `plants` at the foot of the trunk (fungi), each side with
    /// chance `cover`.
    Foot { plants: Vec<Select>, cover: f32 },
}

/// A decoration in the shared pool.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GroundDecorationDef {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub rarity: Rarity,
    pub shape: Shape,
    /// Placed decorations: about one every this many blocks in each direction where it is
    /// densest. Ornaments ignore it.
    #[serde(default)]
    pub spacing: Option<f32>,
    /// Ornaments: the share of matching trees it decorates where it is densest.
    #[serde(default)]
    pub chance: Option<f32>,
    /// Density ramps, multiplied together.
    #[serde(default)]
    pub follows: Vec<Follow>,
    #[serde(default)]
    pub comfort: Comfort,
    /// The ground it stands on; each shape has a default.
    #[serde(default)]
    pub ground: Option<Vec<Ground>>,
    /// Ornaments: the trees it decorates, by selector.
    #[serde(default)]
    pub trees: Vec<Select>,
}

impl GroundDecorationDef {
    /// Whether it decorates trees rather than standing on its own.
    pub fn ornament(&self) -> bool {
        matches!(self.shape, Shape::Litter { .. } | Shape::LeafPiles { .. } | Shape::Foot { .. })
    }

    /// Farthest it reaches from its position, horizontally, in blocks.
    pub fn reach(&self) -> f32 {
        match &self.shape {
            Shape::Patch { radius, .. } | Shape::Boulder { radius, .. } | Shape::Bush { radius, .. } => radius.1 * 1.35,
            Shape::Rocks { spread, size, .. } => spread + size.1,
            Shape::StandingStones { radius, .. } => radius.1 + 2.0,
            Shape::FairyRing { radius, .. } => radius.1 + 1.0,
            Shape::Scatter { spread, .. } => spread + 1.0,
            // Centred on its position; a thick trunk is three wide, branches stick out two
            // more, and the stump stands up to four blocks behind its foot.
            Shape::FallenLog { length, .. } => length.1 as f32 / 2.0 + 4.0,
            Shape::Stump { .. } | Shape::Litter { .. } | Shape::LeafPiles { .. } | Shape::Foot { .. } => 1.0,
        }
    }

    /// The ground it stands on, given or by shape.
    pub fn grounds(&self) -> Vec<Ground> {
        self.ground.clone().unwrap_or_else(|| match self.shape {
            Shape::Patch { .. } | Shape::Litter { .. } | Shape::LeafPiles { .. } | Shape::Foot { .. } => vec![Ground::Soil],
            Shape::Boulder { .. } | Shape::Rocks { .. } | Shape::StandingStones { .. } => vec![Ground::Soil, Ground::Snow, Ground::Sand, Ground::Bare],
            Shape::FairyRing { .. } | Shape::Scatter { .. } => vec![Ground::Soil],
            Shape::Bush { .. } | Shape::FallenLog { .. } | Shape::Stump { .. } => vec![Ground::Soil, Ground::Snow],
        })
    }
}

// ---------------------------------------------------------------------------------------
// surfaces.ron

/// Surface rules many biomes share (slopes of mixed rock and soil, game trails), included
/// by name.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleSetDef {
    pub name: String,
    pub rules: Vec<RuleDef>,
}

// ---------------------------------------------------------------------------------------
// ambient.ron and atmospheres

/// How ambient particles move.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Motion {
    /// Wander slowly on the air (pollen, motes).
    #[default]
    Drift,
    /// Hang in place, bobbing (fireflies, spores).
    Hover,
    /// Settle downwards (snow, ash).
    Fall,
    /// Float upwards (embers, mist).
    Rise,
    /// Stream along with the wind (dust, sand).
    Blow,
}

/// The time of day a particle set shows at.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum DayTime {
    #[default]
    Always,
    Day,
    Night,
    /// Dawn and dusk.
    Twilight,
}

/// The weather a particle set shows in.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum WeatherWhen {
    #[default]
    Always,
    /// No rain or snow falling.
    Dry,
    /// Rain or snow falling.
    Wet,
}

/// A set of ambient particles, for the client to show around the player.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParticleDef {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub rarity: Rarity,
    /// How many there are where conditions suit them fully, 0 ..= 1 of the most the
    /// client shows.
    pub density: f32,
    /// sRGB.
    pub color: (u8, u8, u8),
    /// Size in blocks.
    #[serde(default = "tenth")]
    pub size: f32,
    #[serde(default)]
    pub motion: Motion,
    /// Gives off its own light.
    #[serde(default)]
    pub glow: bool,
    #[serde(default)]
    pub time: DayTime,
    #[serde(default)]
    pub weather: WeatherWhen,
    #[serde(default)]
    pub comfort: Comfort,
    /// How strongly it follows the aether field, 0 (not at all) ..= 1 (only where aether
    /// is high).
    #[serde(default)]
    pub aether: f32,
    /// Drawn as small winged creatures (butterflies, moths) instead of specks: the client
    /// keeps a few on the wing about the player, `density` times `most` where they suit fully,
    /// `color` their wings' ground and `size` their wingspan.
    #[serde(default)]
    pub wings: Option<WingsDef>,
    /// The share of days (for a `Day` set) or nights (`Night`) it comes out on; the air
    /// tuning's `days_out` or `nights_out` (weather.ron) when left out.
    #[serde(default)]
    pub out: Option<f32>,
}

/// The most of one set of winged creatures the client keeps about the player.
pub const WINGS_MOST: u8 = 12;

/// How a set of winged creatures looks and behaves (`ParticleDef::wings`). They are the
/// client's own: nobody else sees the same ones, and nothing can touch them.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WingsDef {
    /// Most about the player at once, where they suit fully.
    pub most: u8,
    /// The wing pattern, painted in `marks` (sRGB) over the set's `color`.
    pub pattern: WingPattern,
    pub marks: (u8, u8, u8),
    /// Blocks over the ground they flutter at, the least first.
    #[serde(default = "wing_height")]
    pub height: (f32, f32),
    /// Blocks per second.
    #[serde(default = "wing_speed")]
    pub speed: f32,
    /// Blocks they go to and settle on a while, by name or `tag:` (flowers).
    #[serde(default)]
    pub visits: Vec<String>,
    /// Drawn to light: they gather about torches, lamps and fires (moths at night).
    #[serde(default)]
    pub to_light: bool,
}

fn wing_height() -> (f32, f32) {
    (0.5, 2.5)
}

fn wing_speed() -> f32 {
    1.5
}

/// How a winged creature's wings are marked.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum WingPattern {
    /// One colour, `marks` only on the edges.
    #[default]
    Plain,
    /// A few round spots.
    Spots,
    /// A band across both wings.
    Band,
    /// An eye on each wing: a ring of `marks` round a dark centre.
    Eyes,
    /// The wingtips.
    Tips,
    /// Fine veins.
    Veins,
}

/// A biome's air: haze, a nudge to the sky and the colour grading, and ambient particles.
/// Everything left out comes from the climate, so most biomes need nothing here.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AtmosphereDef {
    /// Haze colour, sRGB.
    pub haze: Option<(u8, u8, u8)>,
    /// Haze density relative to the usual, 1.
    pub haze_density: Option<f32>,
    /// A tint multiplied into the sky, sRGB (white leaves it).
    pub sky: Option<(u8, u8, u8)>,
    /// Colour grading: −1 cooler ..= 1 warmer, slight around 0.
    pub warmth: Option<f32>,
    /// Colour grading: saturation relative to the usual, 1.
    pub saturation: Option<f32>,
    /// Particle sets, by selector; every set whose climate fits when left out.
    pub particles: Option<Vec<Select>>,
    /// Tags of particle sets never shown here.
    pub exclude: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// terrain.ron

/// A terrain shape: landforms of one kind that rise wherever the climate falls in its
/// boxes. The boxes are written on the bands of `climate.ron`, like biomes', so a shape and
/// the biomes made for it share their edges, while the terrain never asks which biome won.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapeDef {
    pub name: String,
    pub kind: ShapeKind,
    /// The climate it rises in; everywhere when left out.
    #[serde(default)]
    pub climate: Vec<ClimateBox>,
    /// Only over rock provinces with this name or tag.
    #[serde(default)]
    pub province: Option<String>,
    /// How much of its climate it takes: all of it when common, a few regions of it when
    /// rarer; for shapes that stand alone (volcanoes, arches), how many there are.
    #[serde(default)]
    pub rarity: Rarity,
    /// The ground it makes where it stands tallest, over the biome's.
    #[serde(default)]
    pub surface: Option<SurfaceDef>,
    /// Banded rock for its surface's `Strata`.
    #[serde(default)]
    pub strata: Vec<String>,
}

/// Tarns: small still lakes in hollows high in the hills, which the stream tier of the
/// rivers fills where it would otherwise breach a small pit (see `crate::water`). Nothing is
/// carved for them: the land's own hollows hold them. A later package sets only the
/// fields it changes, including individual fields of tarn country.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TarnsDef {
    /// The climate they lie in, on the bands of `climate.ron`, as for shapes.
    pub climate: Vec<ClimateBox>,
    /// The lowest a pit's bottom may lie, in blocks above the sea: tarns are a highland
    /// thing, and lowland water never changes for them.
    pub floor: f32,
    /// Depth of a pit under its rim, in blocks, from which it may hold one.
    pub depth: f32,
    /// The smallest and largest span of the natural water footprint, in blocks.
    pub size: (i32, i32),
    /// The chance that such a pit in tarn country holds one, where the climate keeps lakes
    /// full; less in drier climates, none in deserts.
    pub chance: f32,
    /// Tarn country: a slow noise about `scale` blocks across gathers tarns into `share`
    /// of their climate, a lake district in places and dry rock in others; none elsewhere.
    pub country: TarnCountryDef,
}

layered!(TarnsSet for TarnsDef {
    replace { climate: Vec<ClimateBox>, floor: f32, depth: f32, size: (i32, i32), chance: f32 }
    nested { country: TarnCountrySet }
});

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TarnCountryDef {
    pub scale: f32,
    pub share: f32,
}

layered!(TarnCountrySet for TarnCountryDef { replace { scale: f32, share: f32 } });

/// What a shape makes, and its sizes in blocks.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub enum ShapeKind {
    /// Rounded hills with creased valleys between them.
    Hills { height: f32, spacing: f32 },
    /// Dunes across the prevailing wind: a gentle slope up the windward side and a steep
    /// lee face, in fields with calm margins.
    Dunes { height: f32, spacing: f32 },
    /// Flat-topped tiers with steep risers, stepping down towards the valleys.
    Mesas { height: f32, spacing: f32 },
    /// Gorges cut into high ground, with stepped walls.
    Canyons { depth: f32, width: f32 },
    /// Steep-sided towers with rounded tops, standing in the plain.
    Towers { height: f32, spacing: f32 },
    /// Clusters of thin rock spikes.
    Spires { height: f32, spacing: f32 },
    /// Cones with a breached crater, gathered in volcanic fields, as many and as tall as
    /// the world's volcanism allows.
    Volcanoes { height: f32, radius: f32 },
    /// Natural arches: short, thick spans of rock up to about `span` blocks long, each with
    /// a window through its middle, one at a site (sometimes two or three, very rarely a
    /// field of them), sites standing alone like volcanoes.
    Arches { span: f32, height: f32 },
    /// Cliffs where upland meets the sea, undercut at the waterline, with stacks offshore.
    SeaCliffs { height: f32 },
    /// Broad valleys drowned by the sea where they reach a rugged coast, `depth` below it.
    Fjords { depth: f32 },
}

impl ShapeKind {
    /// Its sizes, which must all be positive.
    fn sizes(&self) -> Vec<f32> {
        match *self {
            ShapeKind::Hills { height, spacing }
            | ShapeKind::Dunes { height, spacing }
            | ShapeKind::Mesas { height, spacing }
            | ShapeKind::Towers { height, spacing }
            | ShapeKind::Spires { height, spacing } => vec![height, spacing],
            ShapeKind::Canyons { depth, width } => vec![depth, width],
            ShapeKind::Volcanoes { height, radius } => vec![height, radius],
            ShapeKind::Arches { span, height } => vec![span, height],
            ShapeKind::SeaCliffs { height } => vec![height],
            ShapeKind::Fjords { depth } => vec![depth],
        }
    }
}

// ---------------------------------------------------------------------------------------
// land.ron

/// The land: the large noise fields every position's parameters come from, and the fields
/// derived from them, ending in the terrain's height (see `crate::router`). There is only
/// one land: a later package sets it field by field ([`Layered`]), a noise, warp or field
/// by its name.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandDef {
    /// Domain warps, by name: smooth offsets that bend the noises that name them.
    #[serde(default)]
    pub warps: Vec<(String, LandWarpDef)>,
    /// Noise fields, by name, each seeded by its name.
    pub noises: Vec<(String, LandNoiseDef)>,
    /// Derived fields, in order: each may read noises, built-in inputs and the fields
    /// before it. The generator reads some of them by name (see `crate::router::Outputs`).
    pub fields: Vec<(String, ExprDef)>,
    /// Heights above the sea are multiplied by this...
    pub vertical_scale: f32,
    /// ...and stretched by `1 + boost · (h / 128)²`, so lowlands stay low while ranges rise.
    pub elevation_boost: f32,
    /// Heights more than `knee` blocks above the sea ease towards `knee + room` instead of
    /// rising further, so peaks round off under the top of the world.
    pub ceiling: (f32, f32),
    /// The 3D noise the field `crag` moves the ground by (see `crate::density`).
    #[serde(default)]
    pub density: DensityDef,
}

layered!(LandSet for LandDef {
    replace { vertical_scale: f32, elevation_boost: f32, ceiling: (f32, f32) }
    nested { density: DensitySet }
    named { warps: LandWarpDef, noises: LandNoiseDef, fields: ExprDef }
});

/// The 3D noise of the land's density.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DensityDef {
    /// Wavelength of its first octave, in blocks.
    pub wavelength: f32,
    pub octaves: i32,
    /// Heights are multiplied by this before sampling: above 1, the noise changes faster up
    /// than across, so rock breaks into ledges; below 1, into buttresses.
    pub squash: f32,
    /// How much more slowly the density falls above the surface than below it: the smaller,
    /// the further rock reaches out over air.
    pub above: f32,
}

layered!(DensitySet for DensityDef { replace { wavelength: f32, octaves: i32, squash: f32, above: f32 } });

impl Default for DensityDef {
    fn default() -> Self {
        Self { wavelength: 40.0, octaves: 2, squash: 1.5, above: 0.35 }
    }
}

/// A domain warp: positions are moved by up to `amplitude` blocks by a smooth noise.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandWarpDef {
    pub wavelength: f32,
    pub amplitude: f32,
    #[serde(default = "three_octaves")]
    pub octaves: i32,
}

fn three_octaves() -> i32 {
    3
}

/// A 2D noise field: octaves of OpenSimplex2, each twice the frequency and half the
/// amplitude of the one before, times its weight; normalised to a standard deviation of
/// `spread`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandNoiseDef {
    /// Wavelength of the first octave, in blocks.
    pub wavelength: f32,
    /// The weight of each octave, from the first.
    pub octaves: Vec<f32>,
    #[serde(default = "unit_spread")]
    pub spread: f32,
    /// The warp that bends it, by name.
    #[serde(default)]
    pub warp: Option<String>,
    /// A world dial its wavelength scales with: `continent_size`.
    #[serde(default)]
    pub scale_by: Option<String>,
}

fn unit_spread() -> f32 {
    1.0
}

/// An expression over fields: a number, the name of a noise, field or built-in input, or an
/// operation.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ExprDef {
    Const(f32),
    Name(String),
    Op(OpDef),
}

#[derive(Clone, Debug, Deserialize)]
pub enum OpDef {
    /// A cubic Hermite spline over `on`: knots `(at, value)` or `(at, value, slope)`, where
    /// each value may itself be an expression (Minecraft's nested terrain splines). Linear
    /// past both ends, along the end slopes.
    Spline { on: Box<ExprDef>, points: Vec<KnotDef> },
    Add(Vec<ExprDef>),
    /// Stops at the first factor that is zero, so a gate in front spares what follows.
    Mul(Vec<ExprDef>),
    Min(Vec<ExprDef>),
    Max(Vec<ExprDef>),
    Abs(Box<ExprDef>),
    Square(Box<ExprDef>),
    Clamp(Box<ExprDef>, f32, f32),
    /// The noise named, sampled at the position moved by (dx, dz) blocks.
    Shifted(String, Box<ExprDef>, Box<ExprDef>),
}

/// A knot of a spline: where it is, its value and its slope (0 when left out).
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum KnotDef {
    Flat(f32, ExprDef),
    Sloped(f32, ExprDef, f32),
}

impl KnotDef {
    pub fn parts(&self) -> (f32, &ExprDef, f32) {
        match self {
            KnotDef::Flat(at, value) => (*at, value, 0.0),
            KnotDef::Sloped(at, value, slope) => (*at, value, *slope),
        }
    }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapeFile<T> {
    shapes: Vec<ShapeDef>,
    tarns: Option<T>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeFile {
    /// How densely trees grow in forests, against the base game's (1); a later package's
    /// replaces an earlier one's.
    #[serde(default)]
    density: Option<f32>,
    /// Where the woods come from; each package's add to the earlier ones'.
    #[serde(default)]
    woods: WoodsDef,
    trees: Vec<TreeDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlantFile {
    /// How densely small plants grow (flowers, ground plants, grasses and ferns), against
    /// the base game's (1); a later package's replaces an earlier one's.
    #[serde(default)]
    density: Option<f32>,
    /// Plants the density leaves at their biomes' own rate (by tag or name): the ones
    /// gathered for food, crafting and alchemy, so thinning the look never thins them.
    #[serde(default)]
    density_keeps: Vec<Select>,
    plants: Vec<PlantDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AmbientFile {
    particles: Vec<ParticleDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecorationFile {
    decorations: Vec<GroundDecorationDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceFile {
    sets: Vec<RuleSetDef>,
}

/// Everything the world generator reads from content packages.
#[derive(Clone, Debug)]
pub struct WorldContent {
    pub climate: ClimateDef,
    pub provinces: Vec<ProvinceDef>,
    pub biomes: Vec<BiomeDef>,
    pub trees: Vec<TreeDef>,
    /// How densely trees grow in forests, a factor on every biome's tree cover (`density`
    /// in `trees.ron`).
    pub tree_density: f32,
    /// Where each wood comes from (`woods` in `trees.ron`).
    pub woods: WoodsDef,
    /// How densely small plants grow, from `plants.ron` (1 when no package sets it).
    pub plant_density: f32,
    /// Plants `plant_density` leaves alone, by tag or name.
    pub plant_density_keeps: Vec<Select>,
    pub plants: Vec<PlantDef>,
    pub particles: Vec<ParticleDef>,
    pub shapes: Vec<ShapeDef>,
    pub decorations: Vec<GroundDecorationDef>,
    /// Shared surface rule sets, already included into the biomes that name them.
    pub rule_sets: Vec<RuleSetDef>,
    /// Ground blocks and the snowy blocks snow lying on them turns them into, from
    /// `weather.ron`: generated snow uses the same table as snow that falls in play.
    pub snow_cover: Vec<(String, String)>,
    pub trails: TrailsDef,
    pub land: LandDef,
    /// Ore veins (`ores.ron`).
    pub ores: crate::ores::OresDef,
    /// Locked treasure chests the land keeps (`hoards.ron`).
    pub hoards: crate::hoards::HoardsDef,
    /// Settlements (`settlements.ron`).
    pub settlements: crate::settlements::SettlementsDef,
    /// Bosses' lairs (`lairs.ron`), every package's.
    pub lairs: Vec<crate::lairs::LairDef>,
    /// Tarns (`terrain.ron`); none when no package defines them.
    pub tarns: Option<TarnsDef>,
    /// What loading noticed and allowed, for the log: a later package replacing data an
    /// earlier one set, and what it replaced.
    pub notes: Vec<String>,
}

/// What hiking trails are made of (`trails.ron`); a later package sets it field by field
/// ([`Layered`]).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrailsDef {
    /// Where trails are laid: near settlements (the default), or everywhere.
    #[serde(default)]
    pub laid: TrailsWhere,
    /// The first rule whose tags the biome has picks the tread.
    pub tread: Vec<TreadDef>,
    /// What grass at the edge of the tread is worn to.
    pub edge: String,
    /// The deck of a bridge, of a short bridge over a creek, and the stones of a ford.
    pub bridge: String,
    pub log: String,
    pub stepping_stone: String,
    /// The stones of cairns and of waymarkers.
    pub cairn: Vec<Select>,
    pub waymarker: Vec<Select>,
}

layered!(TrailsSet for TrailsDef {
    replace { laid: TrailsWhere, tread: Vec<TreadDef>, edge: String, bridge: String, log: String, stepping_stone: String, cairn: Vec<Select>, waymarker: Vec<Select> }
});

/// Where hiking trails are laid.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum TrailsWhere {
    /// Over all the land, between its summits, lakes, waterfalls and trailheads.
    Everywhere,
    /// Only within reach of a settlement. There are none yet, so no trails at all.
    #[default]
    NearSettlements,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TreadDef {
    /// Any of these biome tags; none matches every biome.
    #[serde(default)]
    pub biome: Vec<String>,
    pub blocks: Vec<Select>,
}

/// The base package's files, built in so that a generator can be made from a block
/// registry alone (distant terrain, tools and tests).
pub(crate) const BASE_FILES: [(&str, &str); 19] = [
    ("ambient.ron", include_str!("../../../assets/content/base/ambient.ron")),
    ("crops.ron", include_str!("../../../assets/content/base/crops.ron")),
    ("biomes.ron", include_str!("../../../assets/content/base/biomes.ron")),
    ("cave_biomes.ron", include_str!("../../../assets/content/base/cave_biomes.ron")),
    ("climate.ron", include_str!("../../../assets/content/base/climate.ron")),
    ("cultures.ron", include_str!("../../../assets/content/base/cultures.ron")),
    ("decorations.ron", include_str!("../../../assets/content/base/decorations.ron")),
    ("hoards.ron", include_str!("../../../assets/content/base/hoards.ron")),
    ("lairs.ron", include_str!("../../../assets/content/base/lairs.ron")),
    ("land.ron", include_str!("../../../assets/content/base/land.ron")),
    ("ores.ron", include_str!("../../../assets/content/base/ores.ron")),
    ("plants.ron", include_str!("../../../assets/content/base/plants.ron")),
    ("rocks.ron", include_str!("../../../assets/content/base/rocks.ron")),
    ("settlements.ron", include_str!("../../../assets/content/base/settlements.ron")),
    ("surfaces.ron", include_str!("../../../assets/content/base/surfaces.ron")),
    ("terrain.ron", include_str!("../../../assets/content/base/terrain.ron")),
    ("trails.ron", include_str!("../../../assets/content/base/trails.ron")),
    ("trees.ron", include_str!("../../../assets/content/base/trees.ron")),
    ("weather.ron", include_str!("../../../assets/content/base/weather.ron")),
];

impl WorldContent {
    /// The generator's definitions from loaded content packages, checked against their
    /// blocks.
    pub fn load(content: &Content) -> Result<Self, ContentError> {
        let world = Self::from_files(content.files.iter(), &content.blocks)?;
        world.settlements.check_furnishings(&content.blocks)?;
        world.settlements.people.check_against(&content.creatures, &world.settlements.works())?;
        world.settlements.interiors.check_who(&world.settlements.people.species(), &world.settlements.works())?;
        if let Some(lair) = world.lairs.iter().find(|l| content.creatures.id(&l.boss).is_none()) {
            return Err(ContentError(format!("lairs.ron: lair `{}` is the home of `{}`, which no package has", lair.name, lair.boss)));
        }
        Ok(world)
    }

    /// A fingerprint of what decides what land is made: the generator's files from every
    /// package (mods too), in package order, and every block's name and tags in id order
    /// (land picks blocks by tag). A world records it, so it can tell when land made from
    /// now on may not line up with land made before. Ambient particles change no land and
    /// are left out.
    pub fn fingerprint<'a>(files: impl Iterator<Item = &'a ContentFile>, registry: &BlockRegistry) -> u64 {
        let shapes_land = |name: &str| name != "ambient.ron" && BASE_FILES.iter().any(|(base, _)| *base == name);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut add = |text: &str| {
            for b in text.bytes().chain([0]) {
                hash = (hash ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        };
        for file in files.filter(|file| shapes_land(&file.name)) {
            [&file.package, &file.name, &file.source].into_iter().for_each(|text| add(text));
        }
        for id in 0..registry.len() {
            let def = registry.def(voxmar_engine::block::BlockId(id as u16));
            let mut tags: Vec<&str> = def.tags.iter().map(String::as_str).collect();
            tags.sort_unstable();
            add(&def.name);
            tags.into_iter().for_each(&mut add);
            add("");
        }
        hash
    }

    /// The base package's definitions alone.
    pub fn base(registry: &BlockRegistry) -> Result<Self, ContentError> {
        let files: Vec<ContentFile> =
            BASE_FILES.iter().map(|(name, source)| ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() }).collect();
        Self::from_files(files.iter(), registry)
    }

    /// Parses definition files given in package order. Single-valued data (`climate.ron`,
    /// `land.ron`, `trails.ron`, terrain's tarn settings, the tree and plant densities) a later package sets over an
    /// earlier one's, field by field, with a note; other definitions accumulate.
    pub fn from_files<'a>(files: impl Iterator<Item = &'a ContentFile>, registry: &BlockRegistry) -> Result<Self, ContentError> {
        let mut climate: Option<(ClimateDef, Setters)> = None;
        let mut trails: Option<(TrailsDef, Setters)> = None;
        let mut land: Option<(LandDef, Setters)> = None;
        let mut tree_density: Option<(f32, String)> = None;
        let mut notes = Vec::new();
        let mut woods = WoodsDef::default();
        let mut plant_density: Option<(f32, String)> = None;
        let mut plant_density_keeps = Vec::new();
        let mut provinces = Vec::new();
        let (mut trees, mut plants, mut particles, mut shapes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut decorations, mut rule_sets) = (Vec::new(), Vec::new());
        let mut weather = voxmar_engine::weather::WeatherTable::default();
        let mut ores = crate::ores::OresDef::default();
        let mut hoards = crate::hoards::HoardsDef::default();
        let mut settlements = crate::settlements::SettlementsDef::default();
        let mut lairs = Vec::new();
        let mut tarns: Option<(TarnsDef, Setters)> = None;
        let mut raw = Vec::new();
        for file in files {
            let at = format!("{}/{}", file.package, file.name);
            let q = |name: &str| qualify(&file.package, name);
            match file.name.as_str() {
                "climate.ron" => lay_file(
                    &mut climate,
                    file,
                    |set: &mut ClimateSet| {
                        if let Some(f) = &mut set.frost {
                            for name in [&mut f.snow, &mut f.powder, &mut f.snowy_ground, &mut f.ice, &mut f.thick_ice, &mut f.rare_ice].into_iter().flatten() {
                                *name = q(name);
                            }
                            for b in f.blanket.iter_mut().flatten() {
                                *b = BlanketDef { covers: qualify_match(&file.package, &b.covers), to: q(&b.to), not: b.not.clone() };
                            }
                        }
                    },
                    &mut notes,
                )?,
                "rocks.ron" => provinces.extend(parse::<ProvinceFile>(&at, &file.source)?.provinces.into_iter().map(|p| ProvinceDef {
                    name: q(&p.name),
                    stones: p.stones.iter().map(|s| qualify_select(&file.package, s)).collect(),
                    ..p
                })),
                "biomes.ron" | "cave_biomes.ron" => {
                    for biome in parse::<BiomeFile>(&at, &file.source)?.biomes {
                        raw.push((at.clone(), qualify_biome(&file.package, biome)));
                    }
                }
                "trees.ron" => {
                    let def = parse::<TreeFile>(&at, &file.source)?;
                    if let Some(density) = def.density {
                        if !(density > 0.0 && density <= 4.0) {
                            return Err(ContentError(format!("{at}: the tree density {density} is outside (0, 4]")));
                        }
                        if let Some((earlier, by)) = tree_density.replace((density, file.package.clone())) {
                            notes.push(format!("{at} replaces the tree density {by} set ({earlier} to {density})"));
                        }
                    }
                    let q = |n: &str| qualify(&file.package, n);
                    woods.add(&at, &file.package, &WoodsDef {
                        common: def.woods.common.iter().map(|n| q(n)).collect(),
                        families: def.woods.families.iter().map(|(wood, family)| (q(wood), family.iter().map(|n| q(n)).collect())).collect(),
                    }, &mut notes)?;
                    trees.extend(def.trees.into_iter().map(|t| {
                        TreeDef {
                            name: q(&t.name),
                            log: q(&t.log),
                            leaves: t.leaves.as_deref().map(q),
                            alternate: t.alternate.as_ref().map(|(n, share)| (q(n), *share)),
                            root_block: t.root_block.as_deref().map(q),
                            fruit: t.fruit.as_ref().map(|f| Fruit { leaves: q(&f.leaves), share: f.share }),
                            ..t
                        }
                    }));
                }
                "ambient.ron" => particles.extend(
                    parse::<AmbientFile>(&at, &file.source)?.particles.into_iter().map(|p| ParticleDef { name: qualify(&file.package, &p.name), ..p }),
                ),
                "plants.ron" => {
                    let def = parse::<PlantFile>(&at, &file.source)?;
                    if let Some(density) = def.density {
                        if !(density > 0.0 && density <= 4.0) {
                            return Err(ContentError(format!("{at}: the plant density {density} is outside (0, 4]")));
                        }
                        if let Some((earlier, by)) = plant_density.replace((density, file.package.clone())) {
                            notes.push(format!("{at} replaces the plant density {by} set ({earlier} to {density})"));
                        }
                    }
                    plant_density_keeps.extend(def.density_keeps.iter().map(|s| match s {
                        Select::Name(n, w) => Select::Name(qualify(&file.package, n), *w),
                        other => other.clone(),
                    }));
                    plants.extend(def.plants.into_iter().map(|p| {
                        let q = |n: &str| qualify(&file.package, n);
                        PlantDef { name: q(&p.name), block: q(&p.block), top: p.top.as_deref().map(q), snowy: p.snowy.as_deref().map(q), ..p }
                    }));
                }
                "terrain.ron" => {
                    let file_shapes = match &mut tarns {
                        None => {
                            let def = parse::<ShapeFile<TarnsDef>>(&at, &file.source)?;
                            if let Some(mut whole) = def.tarns {
                                let mut changed = Vec::new();
                                whole.lay(whole.clone().into_set(), "tarns.", &mut changed);
                                tarns = Some((whole, changed.into_iter().map(|path| (path, file.package.clone())).collect()));
                            }
                            def.shapes
                        }
                        Some((whole, setters)) => {
                            let def = parse::<ShapeFile<TarnsSet>>(&at, &file.source)?;
                            if let Some(set) = def.tarns {
                                lay_set(whole, setters, set, file, "tarns.", &mut notes);
                            }
                            def.shapes
                        }
                    };
                    shapes.extend(file_shapes.into_iter().map(|s| ShapeDef {
                        name: q(&s.name),
                        surface: s.surface.map(|surface| qualify_surface(&file.package, surface)),
                        strata: s.strata.iter().map(|n| q(n)).collect(),
                        ..s
                    }));
                }
                "land.ron" => lay_file(&mut land, file, |_: &mut LandSet| {}, &mut notes)?,
                "ores.ron" => ores.extend(&at, &file.package, &file.source)?,
                "hoards.ron" => hoards.extend(&at, &file.package, &file.source)?,
                "settlements.ron" => settlements.extend(&at, &file.package, &file.source)?,
                "lairs.ron" => lairs.extend(crate::lairs::read(&at, &file.package, &file.source)?),
                "cultures.ron" => settlements.cultures.extend(&at, &file.package, &file.source)?,
                // Each crop's level, rarity and stages, so settlements keep to everyday crops
                // and never grow them ripe.
                "crops.ron" => {
                    #[derive(serde::Deserialize)]
                    struct Crop {
                        name: String,
                        #[serde(default)]
                        level: u32,
                        #[serde(default)]
                        rarity: voxmar_engine::block::Rarity,
                        #[serde(default)]
                        stages: Vec<String>,
                    }
                    #[derive(serde::Deserialize)]
                    struct Crops {
                        crops: Vec<Crop>,
                    }
                    for c in parse::<Crops>(&at, &file.source)?.crops {
                        let name = q(&c.name);
                        let stages = if c.stages.is_empty() { (0..3).map(|i| format!("{name}_crop_{i}")).chain([format!("{name}_crop")]).collect() } else { c.stages.iter().map(|s| q(s)).collect() };
                        settlements.crops.insert(name, crate::settlements::def::CropInfo { level: c.level, rarity: c.rarity, stages });
                    }
                }
                "weather.ron" => weather.extend(&file.package, &file.source).map_err(|e| ContentError(e.0))?,
                "decorations.ron" => decorations.extend(
                    parse::<DecorationFile>(&at, &file.source)?.decorations.into_iter().map(|d| qualify_decoration(&file.package, d)),
                ),
                "surfaces.ron" => rule_sets.extend(parse::<SurfaceFile>(&at, &file.source)?.sets.into_iter().map(|set| RuleSetDef {
                    name: qualify(&file.package, &set.name),
                    rules: set.rules.into_iter().map(|r| RuleDef { when: r.when, place: qualify_layers(&file.package, r.place) }).collect(),
                })),
                "trails.ron" => lay_file(
                    &mut trails,
                    file,
                    |set: &mut TrailsSet| {
                        let stones = |s: &[Select]| s.iter().map(|s| qualify_select(&file.package, s)).collect::<Vec<_>>();
                        for tread in set.tread.iter_mut().flatten() {
                            tread.blocks = stones(&tread.blocks);
                        }
                        for name in [&mut set.edge, &mut set.bridge, &mut set.log, &mut set.stepping_stone].into_iter().flatten() {
                            *name = q(name);
                        }
                        for list in [&mut set.cairn, &mut set.waymarker].into_iter().flatten() {
                            *list = stones(list);
                        }
                    },
                    &mut notes,
                )?,
                _ => {}
            }
        }
        let climate = climate.map(|(c, _)| c).ok_or_else(|| ContentError("no content package defines climate.ron".into()))?;
        let trails = trails.map(|(t, _)| t).ok_or_else(|| ContentError("no content package defines trails.ron".into()))?;
        let land = land.map(|(l, _)| l).ok_or_else(|| ContentError("no content package defines land.ron".into()))?;
        let mut biomes = inherit(&raw)?;
        // Shared rule sets come before each biome's own rules.
        for biome in &mut biomes {
            let mut rules = Vec::new();
            for name in &biome.surface.include {
                let Some(set) = rule_sets.iter().find(|s: &&RuleSetDef| s.name == *name) else {
                    let at = raw.iter().find(|(_, b)| b.name == biome.name).map_or("?", |(at, _)| at.as_str());
                    return Err(ContentError(format!("{at}: biome `{}`: unknown surface rule set `{name}`", biome.name)));
                };
                rules.extend(set.rules.iter().cloned());
            }
            rules.append(&mut biome.surface.rules);
            biome.surface.rules = rules;
        }
        // Terrain shapes' surfaces take shared rule sets the same way.
        for shape in &mut shapes {
            let Some(surface) = &mut shape.surface else { continue };
            let mut rules = Vec::new();
            for name in &surface.include {
                let Some(set) = rule_sets.iter().find(|s: &&RuleSetDef| s.name == *name) else {
                    return Err(ContentError(format!("terrain.ron: shape `{}`: unknown surface rule set `{name}`", shape.name)));
                };
                rules.extend(set.rules.iter().cloned());
            }
            rules.append(&mut surface.rules);
            surface.rules = rules;
        }
        let snow_cover = weather.snow_cover().to_vec();
        let tree_density = tree_density.map_or(1.0, |(d, _)| d);
        let plant_density = plant_density.map_or(1.0, |(d, _)| d);
        notes.extend(settlements.cultures.notes.iter().cloned());
        notes.extend(settlements.people.notes.iter().cloned());
        for note in &notes {
            tracing::info!("{note}");
        }
        let content = Self {
            climate,
            provinces,
            biomes,
            trees,
            tree_density,
            woods,
            plant_density,
            plant_density_keeps,
            plants,
            particles,
            shapes,
            decorations,
            rule_sets,
            snow_cover,
            trails,
            land,
            ores,
            hoards,
            settlements,
            lairs,
            tarns: tarns.map(|(t, _)| t),
            notes,
        };
        content.ores.check(registry, &content.provinces)?;
        content.hoards.check(registry)?;
        if !content.settlements.styles.is_empty() {
            content.settlements.check(registry)?;
        }
        content.check(registry, &raw.iter().map(|(at, b)| (b.name.clone(), at.clone())).collect::<Vec<_>>())?;
        for lair in &content.lairs {
            lair.check(registry, |name| content.biome(name).is_some()).map_err(ContentError)?;
        }
        // Every plant the density keeps names one: a typo would thin what it meant to keep.
        for keep in &content.plant_density_keeps {
            let found = match keep {
                Select::Name(name, _) => content.plants.iter().any(|p| p.name == *name),
                Select::Tag(tag, _) => content.plants.iter().any(|p| content.plant_tags(p, registry).any(|t| t == tag)),
            };
            if !found {
                return Err(ContentError(format!("plants.ron: density_keeps names {keep:?}, which no plant is or carries")));
            }
        }
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

        // Trails.
        let trails_at = || "trails.ron".to_owned();
        if !self.trails.tread.last().is_some_and(|t| t.biome.is_empty()) {
            return Err(ContentError(format!("{}: the last tread rule must match every biome (no tags)", trails_at())));
        }
        for tread in &self.trails.tread {
            blocks(&trails_at, &tread.blocks)?;
        }
        block(&trails_at, &self.trails.edge)?;
        block(&trails_at, &self.trails.bridge)?;
        block(&trails_at, &self.trails.log)?;
        blocks(&trails_at, &self.trails.cairn)?;
        blocks(&trails_at, &self.trails.waymarker)?;
        block(&trails_at, &self.trails.stepping_stone)?;

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
        self.climate.levels.check().map_err(|e| ContentError(format!("climate.ron: {e}")))?;
        for tag in &self.climate.levels.trees {
            if !self.trees.iter().any(|t| t.tags.contains(tag)) {
                return Err(ContentError(format!("climate.ron: levels.trees names the tag `{tag}`, which no tree carries")));
            }
        }
        let frost = &self.climate.frost;
        for name in [&frost.snow, &frost.powder, &frost.snowy_ground, &frost.ice, &frost.thick_ice, &frost.rare_ice] {
            block(&climate_at, name)?;
        }
        for (ground, snowy) in &self.snow_cover {
            block(&|| "weather.ron: snow_cover".to_owned(), ground)?;
            block(&|| "weather.ron: snow_cover".to_owned(), snowy)?;
        }
        for rule in &frost.blanket {
            block(&climate_at, &rule.to)?;
            match &rule.covers {
                BlockMatch::Name(name) => block(&climate_at, name)?,
                BlockMatch::Tag(tag) if registry.tagged(tag).next().is_none() => {
                    return Err(ContentError(format!("climate.ron: no block has the tag `{tag}` the snow blanket covers")));
                }
                BlockMatch::Tag(_) => {}
            }
            if let Some(tag) = rule.not.iter().find(|tag| registry.tagged(tag).next().is_none()) {
                return Err(ContentError(format!("climate.ron: no block has the tag `{tag}` the snow blanket spares")));
            }
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
            if !(1..=MAX_LEVEL).contains(&biome.level) {
                return Err(ContentError(format!("{}: level {} is not within 1 ..= {MAX_LEVEL}", at(), biome.level)));
            }
            for climate in &biome.climate {
                self.check_box(climate, &at)?;
                let underground = climate.depth.is_some() || climate.height.is_some() || climate.provinces.is_some();
                if underground && biome.layer != Layer::Underground {
                    return Err(ContentError(format!("{}: depth, height and provinces are for underground biomes", at())));
                }
                for (what, range) in [("depth", climate.depth), ("height", climate.height)] {
                    if let Some((lo, hi)) = range
                        && lo.partial_cmp(&hi) != Some(std::cmp::Ordering::Less)
                    {
                        return Err(ContentError(format!("{}: the {what} range ({lo}, {hi}) is empty", at())));
                    }
                }
                for name in climate.provinces.iter().flatten() {
                    if !self.provinces.iter().any(|p| p.matches(name)) {
                        return Err(ContentError(format!("{}: no rock province is named or tagged `{name}`", at())));
                    }
                }
            }
            if !biome.cave_decoration.is_empty() && biome.layer != Layer::Underground {
                return Err(ContentError(format!("{}: cave decoration is for underground biomes", at())));
            }
            for (i, decoration) in biome.cave_decoration.iter().enumerate() {
                let at = || format!("{}, cave decoration {}", at(), i + 1);
                self.check_decoration(decoration, !biome.strata.is_empty(), registry, &at, &block, &blocks)?;
            }
            let is_variant = self.biomes.iter().any(|b| b.variants.iter().any(|(v, _)| *v == biome.name));
            if biome.layer == Layer::Surface && biome.climate.is_empty() && biome.ecotone.is_none() && !is_variant {
                return Err(ContentError(format!("{}: never grows: it has no climate, is no ecotone and no biome's variant", at())));
            }
            if biome.layer == Layer::Underground && (biome.climate.is_empty() && !is_variant || biome.ecotone.is_some()) {
                return Err(ContentError(format!("{}: an underground biome grows in a climate box or as a variant, not as an ecotone", at())));
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

        if let Some(tarns) = &self.tarns {
            let at = || "terrain.ron: tarns".to_owned();
            if !(1..=64).contains(&tarns.size.0) || !(tarns.size.0..=64).contains(&tarns.size.1) {
                return Err(ContentError(format!("{}: size {:?} must increase within 1..=64 blocks", at(), tarns.size)));
            }
            for climate in &tarns.climate {
                if climate.aether.is_some() || climate.depth.is_some() || climate.height.is_some() || climate.provinces.is_some() {
                    return Err(ContentError(format!("{}: tarns follow the climate bands alone", at())));
                }
                self.check_box(climate, &at)?;
            }
            let TarnCountryDef { scale, share } = tarns.country;
            for (what, value, range) in [("floor", tarns.floor, 0.0..=256.0), ("depth", tarns.depth, 0.5..=16.0), ("chance", tarns.chance, 0.0..=1.0), ("country scale", scale, 64.0..=16384.0), ("country share", share, 0.0..=1.0)] {
                if !range.contains(&value) {
                    return Err(ContentError(format!("{}: the {what} {value} is outside {range:?}", at())));
                }
            }
        }

        // Terrain shapes.
        for (i, shape) in self.shapes.iter().enumerate() {
            let at = || format!("terrain.ron: shape `{}`", shape.name);
            if self.shapes[..i].iter().any(|s| s.name == shape.name) {
                return Err(ContentError(format!("{}: defined twice", at())));
            }
            if shape.kind.sizes().iter().any(|&size| !(size > 0.0 && size.is_finite())) {
                return Err(ContentError(format!("{}: sizes must be positive", at())));
            }
            for climate in &shape.climate {
                if climate.aether.is_some() || climate.depth.is_some() || climate.height.is_some() || climate.provinces.is_some() {
                    return Err(ContentError(format!("{}: shapes follow the climate bands alone (use `province` for rock)", at())));
                }
                self.check_box(climate, &at)?;
            }
            if let Some(province) = &shape.province
                && !self.provinces.iter().any(|p| p.matches(province))
            {
                return Err(ContentError(format!("{}: no rock province is named or tagged `{province}`", at())));
            }
            for name in &shape.strata {
                block(&|| format!("{}, strata", at()), name)?;
            }
            if let Some(surface) = &shape.surface {
                self.check_surface(surface, !shape.strata.is_empty(), &|| at(), &block, &blocks)?;
            }
        }

        self.check_flora(registry, &block, &file_of)?;
        self.check_decorations(&block, &blocks)?;
        for set in &self.rule_sets {
            let at = || format!("surfaces.ron: rule set `{}`", set.name);
            let surface = SurfaceDef {
                rules: set.rules.clone(),
                include: Vec::new(),
                ground: LayersDef { top: Material::Stones, filler: None, depth: None },
                underwater: LayersDef { top: Material::Stones, filler: None, depth: None },
                shore: None,
                cliff: None,
            };
            self.check_surface(&surface, true, &at, &block, &blocks)?;
        }
        self.check_coverage()
    }

    /// Checks a box of the band table: its band names and ranges, and its aether.
    fn check_box(&self, climate: &ClimateBox, at: &dyn Fn() -> String) -> Result<(), ContentError> {
        for axis in [Axis::Temperature, Axis::Moisture, Axis::Continentalness, Axis::Erosion] {
            if let Some((from, to)) = climate.bands(axis) {
                let names = &self.bands(axis).names;
                let index = |band: &str| {
                    names
                        .iter()
                        .position(|n| n == band)
                        .ok_or_else(|| ContentError(format!("{}: unknown {axis:?} band `{band}` (bands: {})", at(), names.join(", "))))
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
        Ok(())
    }

    /// The span of an axis a range of its bands covers, from the lower edge of the first
    /// to the upper edge of the last; infinite at the ends of the axis.
    pub fn span(&self, axis: Axis, bands: Option<&(String, String)>) -> (f32, f32) {
        let def = self.bands(axis);
        let Some((from, to)) = bands else {
            return (f32::NEG_INFINITY, f32::INFINITY);
        };
        let index = |band: &str| def.names.iter().position(|n| n == band).expect("checked at load");
        let (lo, hi) = (index(from), index(to));
        let lower = if lo == 0 { f32::NEG_INFINITY } else { def.edges[lo - 1] };
        let upper = def.edges.get(hi).copied().unwrap_or(f32::INFINITY);
        (lower, upper)
    }

    /// Whether the plant density leaves a plant at its biomes' own rate.
    pub fn plant_kept(&self, plant: &PlantDef, registry: &BlockRegistry) -> bool {
        self.plant_density_keeps.iter().any(|s| match s {
            Select::Name(name, _) => *name == plant.name,
            Select::Tag(tag, _) => self.plant_tags(plant, registry).any(|t| t == tag),
        })
    }

    /// Tags of a plant: its own and its block's.
    pub fn plant_tags<'a>(&'a self, plant: &'a PlantDef, registry: &'a BlockRegistry) -> impl Iterator<Item = &'a str> + Clone + 'a {
        let block = registry.id(&plant.block).map(|id| registry.def(id).tags.as_slice()).unwrap_or_default();
        plant.tags.iter().chain(block).map(String::as_str)
    }

    fn check_flora(&self, registry: &BlockRegistry, block: &BlockCheck<'_, str>, file_of: &dyn Fn(&str) -> String) -> Result<(), ContentError> {
        let comfort = |at: &dyn Fn() -> String, c: &Comfort| {
            for (what, range) in [("temperature", c.temperature), ("moisture", c.moisture)] {
                if let Some((lo, hi)) = range
                    && !(lo.is_finite() && hi.is_finite() && lo < hi)
                {
                    return Err(ContentError(format!("{}: the {what} comfort range ({lo}, {hi}) is empty", at())));
                }
            }
            Ok(())
        };
        for (i, tree) in self.trees.iter().enumerate() {
            let at = || format!("trees.ron: tree `{}`", tree.name);
            if self.trees[..i].iter().any(|t| t.name == tree.name) {
                return Err(ContentError(format!("{}: defined twice", at())));
            }
            block(&at, &tree.log)?;
            for name in tree.leaves.iter().chain(tree.alternate.as_ref().map(|a| &a.0)).chain(&tree.root_block) {
                block(&at, name)?;
            }
            if let Some(fruit) = &tree.fruit {
                block(&at, &fruit.leaves)?;
                if registry.id(&fruit.leaves).is_some_and(|id| registry.info(id).render != RenderKind::Leaves) {
                    return Err(ContentError(format!("{}: its fruit `{}` is not a kind of leaves", at(), fruit.leaves)));
                }
                if !(fruit.share > 0.0 && fruit.share <= 0.5) || tree.leaves.is_none() {
                    return Err(ContentError(format!("{}: fruit needs leaves, and a share in (0, 0.5]", at())));
                }
            }
            if tree.leaves.is_none() && !matches!(tree.crown, Crown::Dead { .. } | Crown::Bare) {
                return Err(ContentError(format!("{}: its crown needs leaves", at())));
            }
            if tree.height.0 < 1 || tree.height.0 > tree.height.1 || tree.height.1 > 40 {
                return Err(ContentError(format!("{}: height must be a range within 1..=40", at())));
            }
            let reach = tree.lean + tree.crown.reach();
            if reach > crate::trees::REACH as f32 {
                return Err(ContentError(format!("{}: reaches {reach} blocks from its trunk, more than {}", at(), crate::trees::REACH)));
            }
            if let Crown::Dead { broken, fork, .. } = tree.crown
                && !((0.0..=1.0).contains(&broken) && (0.0..=1.0).contains(&fork))
            {
                return Err(ContentError(format!("{}: `broken` and `fork` are shares, within 0..=1", at())));
            }
            if tree.ground.is_empty() {
                return Err(ContentError(format!("{}: grows on no ground", at())));
            }
            if let Some((lo, hi)) = tree.stand
                && !(0.0 <= lo && lo < hi && hi <= 1.0)
            {
                return Err(ContentError(format!("{}: the stand range ({lo}, {hi}) is empty or outside 0..=1", at())));
            }
            comfort(&at, &tree.comfort)?;
        }
        for wood in &self.woods.common {
            block(&|| "trees.ron: the common woods".to_string(), wood)?;
        }
        for (wood, family) in &self.woods.families {
            let at = || format!("trees.ron: the family of wood `{wood}`");
            block(&at, wood)?;
            for name in family {
                let Some(tree) = self.trees.iter().find(|d| d.name == *name) else {
                    return Err(ContentError(format!("{}: no tree is named `{name}`", at())));
                };
                if tree.log != *wood && tree.root_block.as_ref() != Some(wood) {
                    return Err(ContentError(format!("{}: `{name}` is not built of `{wood}`", at())));
                }
            }
        }
        for (i, plant) in self.plants.iter().enumerate() {
            let at = || format!("plants.ron: plant `{}`", plant.name);
            if self.plants[..i].iter().any(|p| p.name == plant.name) {
                return Err(ContentError(format!("{}: defined twice", at())));
            }
            block(&at, &plant.block)?;
            if let Some(top) = &plant.top {
                block(&at, top)?;
            }
            if let Some((lo, hi)) = plant.stack
                && (lo < 1 || lo > hi || hi > 8 || plant.top.is_some())
            {
                return Err(ContentError(format!("{}: a stack is 1..=8 blocks tall, and has no top", at())));
            }
            if plant.ground.is_empty() {
                return Err(ContentError(format!("{}: grows on no ground", at())));
            }
            if let Some(snowy) = &plant.snowy {
                block(&at, snowy)?;
            }
            comfort(&at, &plant.comfort)?;
            if let Some(p) = &plant.patches {
                if !(p.rate.is_finite() && p.rate > 0.0) {
                    return Err(ContentError(format!("{}: the patch rate {} must be a number above 0", at(), p.rate)));
                }
                if !(1 <= p.size.0 && p.size.0 <= p.size.1 && p.size.1 <= PATCH_MOST) {
                    return Err(ContentError(format!("{}: a patch has 1 to {PATCH_MOST} plants, the least first, not {:?}", at(), p.size)));
                }
                if !(p.aim.is_finite() && p.aim > 0.0) {
                    return Err(ContentError(format!("{}: the patch aim {} must be a number above 0", at(), p.aim)));
                }
                if !plant.named_only {
                    return Err(ContentError(format!("{}: grows in patches only where named, so it needs `named_only: true`", at())));
                }
            }
        }
        for (i, particles) in self.particles.iter().enumerate() {
            let at = || format!("ambient.ron: particles `{}`", particles.name);
            if self.particles[..i].iter().any(|p| p.name == particles.name) {
                return Err(ContentError(format!("{}: defined twice", at())));
            }
            if !(0.0..=1.0).contains(&particles.density) || !(0.0..=1.0).contains(&particles.aether) || !(particles.size > 0.0 && particles.size <= 4.0) {
                return Err(ContentError(format!("{}: density and aether must be in 0..=1 and size in (0, 4]", at())));
            }
            if particles.out.is_some_and(|out| !(0.0..=1.0).contains(&out)) {
                return Err(ContentError(format!("{}: `out` is a share of days or nights, 0 to 1", at())));
            }
            if let Some(wings) = &particles.wings {
                let (low, high) = wings.height;
                if wings.most == 0 || wings.most > WINGS_MOST || !(0.0 <= low && low <= high && high <= 16.0) || !(wings.speed > 0.0 && wings.speed <= 8.0) || particles.size > 1.0 {
                    return Err(ContentError(format!(
                        "{}: wings need `most` 1 to {WINGS_MOST}, `height` from 0 to 16 blocks with the least first, `speed` above 0 and up to 8, and `size` up to 1",
                        at()
                    )));
                }
                for visit in &wings.visits {
                    let known = match visit.strip_prefix("tag:") {
                        Some(tag) => registry.tagged(tag).next().is_some(),
                        None => registry.id(visit).is_some(),
                    };
                    if !known {
                        return Err(ContentError(format!("{}: visits `{visit}`, which no block is (a block's full name, or `tag:` and a tag)", at())));
                    }
                }
            }
            comfort(&at, &particles.comfort)?;
        }
        for biome in &self.biomes {
            let file = file_of(&biome.name);
            let at = || format!("{file}: biome `{}`", biome.name);
            // A patch plant's weight here may make at most one patch a cell (ecology.rs
            // `in_patch`): its rate times the weight, over a mean patch, fills a cell at most.
            // One along water (`edge_patches`) may at most grow on every column of its habitat.
            for select in biome.flora.plants.iter().chain(biome.flora.wonders.iter().flatten()) {
                let Select::Name(name, weight) = select else { continue };
                let Some(plant) = self.plants.iter().find(|p| p.name == *name) else { continue };
                let Some(p) = &plant.patches else { continue };
                if !(weight.is_finite() && *weight > 0.0) {
                    return Err(ContentError(format!("{}: plant `{name}` at weight {weight}: a weight must be above 0", at())));
                }
                if plant.habitat.edge() {
                    let share = p.rate * weight / 1e6;
                    if share > 1.0 {
                        return Err(ContentError(format!("{}: plant `{name}` at weight {weight} asks for {share:.2} plants a column of its habitat; at most 1", at())));
                    }
                    continue;
                }
                let cell = (crate::ecology::PATCH_CELL * crate::ecology::PATCH_CELL) as f32;
                let chance = p.rate * weight * cell / ((p.size.0 + p.size.1) as f32 / 2.0) / 1e6;
                if chance > 1.0 {
                    return Err(ContentError(format!("{}: plant `{name}` at weight {weight} asks for {chance:.2} patches a {}-block cell; at most 1", at(), crate::ecology::PATCH_CELL)));
                }
            }
            let air = &biome.atmosphere;
            for (what, value, range) in [("haze_density", air.haze_density, 0.0..=4.0), ("warmth", air.warmth, -1.0..=1.0), ("saturation", air.saturation, 0.0..=2.0)] {
                if value.is_some_and(|v| !range.contains(&v)) {
                    return Err(ContentError(format!("{}: atmosphere {what} must be in {range:?}", at())));
                }
            }
            for select in air.particles.iter().flatten() {
                positive(&|| format!("{}, atmosphere", at()), "weight", select.weight())?;
                if let Select::Name(name, _) = select
                    && !self.particles.iter().any(|p| p.name == *name)
                {
                    return Err(ContentError(format!("{}, atmosphere: unknown particles `{name}`", at())));
                }
            }
            let flora = &biome.flora;
            for (what, v) in [("forest", flora.forest), ("cover", flora.cover), ("flowers", flora.flowers), ("bloom", flora.bloom)] {
                if !(0.0..=1.0).contains(&v) {
                    return Err(ContentError(format!("{}: {what} must be in 0..=1", at())));
                }
            }
            let check = |list: &str, selects: &[Select], trees: bool, plants: bool| -> Result<(), ContentError> {
                for select in selects {
                    positive(&|| format!("{}, {list}", at()), "weight", select.weight())?;
                    let found = match select {
                        Select::Name(name, _) => {
                            (trees && self.trees.iter().any(|t| t.name == *name)) || (plants && self.plants.iter().any(|p| p.name == *name))
                        }
                        // A tag nothing carries yet is fine: content with it may come later.
                        Select::Tag(..) => true,
                    };
                    if !found {
                        let kind = if trees && plants { "tree or plant" } else if trees { "tree" } else { "plant" };
                        return Err(ContentError(format!("{}, {list}: unknown {kind} `{}`", at(), match select {
                            Select::Name(n, _) | Select::Tag(n, _) => n,
                        })));
                    }
                }
                Ok(())
            };
            check("trees", &flora.trees, true, false)?;
            check("plants", &flora.plants, false, true)?;
            if let Some(wonders) = &flora.wonders {
                check("wonders", wonders, true, true)?;
            }
            for select in flora.decorations.iter().flatten() {
                positive(&|| format!("{}, decorations", at()), "weight", select.weight())?;
                if let Select::Name(name, _) = select
                    && !self.decorations.iter().any(|d| d.name == *name)
                {
                    return Err(ContentError(format!("{}, decorations: unknown decoration `{name}`", at())));
                }
            }
        }
        Ok(())
    }

    fn check_decorations(&self, block: &BlockCheck<'_, str>, blocks: &BlockCheck<'_, [Select]>) -> Result<(), ContentError> {
        for (i, def) in self.decorations.iter().enumerate() {
            let at = || format!("decorations.ron: decoration `{}`", def.name);
            if self.decorations[..i].iter().any(|d| d.name == def.name) {
                return Err(ContentError(format!("{}: defined twice", at())));
            }
            let material = |m: &Material| match m {
                Material::Block(name) => block(&at, name),
                Material::Mix(selects) => blocks(&at, selects),
                Material::Stones => Ok(()),
                Material::Strata => Err(ContentError(format!("{}: decorations have no strata to use", at()))),
            };
            let range = |what: &str, (lo, hi): (f32, f32), max: f32| {
                if lo > 0.0 && lo <= hi && hi <= max {
                    Ok(())
                } else {
                    Err(ContentError(format!("{}: {what} must be a range within (0, {max}]", at())))
                }
            };
            let share = |what: &str, v: f32| {
                if (0.0..=1.0).contains(&v) { Ok(()) } else { Err(ContentError(format!("{}: {what} must be in 0..=1", at()))) }
            };
            let plants = |list: &[Select]| -> Result<(), ContentError> {
                for select in list {
                    positive(&at, "weight", select.weight())?;
                    if let Select::Name(name, _) = select
                        && !self.plants.iter().any(|p| p.name == *name)
                    {
                        return Err(ContentError(format!("{}: unknown plant `{name}`", at())));
                    }
                }
                Ok(())
            };
            match &def.shape {
                Shape::Patch { radius, top } => {
                    range("radius", *radius, 6.0)?;
                    material(top)?;
                }
                Shape::Boulder { radius, stones, moss } => {
                    range("radius", *radius, 4.0)?;
                    material(stones)?;
                    share("moss", *moss)?;
                }
                Shape::Rocks { count, spread, size, stones, moss } => {
                    if count.0 < 1 || count.0 > count.1 || count.1 > 12 {
                        return Err(ContentError(format!("{}: count must be a range within 1..=12", at())));
                    }
                    range("size", *size, 2.0)?;
                    range("spread", (*spread, *spread), 6.0)?;
                    material(stones)?;
                    share("moss", *moss)?;
                }
                Shape::Bush { radius, leaves, log } => {
                    range("radius", *radius, 3.0)?;
                    for name in leaves.iter().chain(log) {
                        block(&at, name)?;
                    }
                }
                Shape::FallenLog { length, thick, hollow, branches, stump, growth, plants: list } => {
                    if length.0 < 2 || length.0 > length.1 || length.1 > 8 {
                        return Err(ContentError(format!("{}: length must be a range within 2..=8", at())));
                    }
                    for (what, v) in [("thick", *thick), ("hollow", *hollow), ("branches", *branches), ("stump", *stump), ("growth", *growth)] {
                        share(what, v)?;
                    }
                    plants(list)?;
                }
                Shape::Stump { height, growth, plants: list } => {
                    if height.0 < 1 || height.0 > height.1 || height.1 > 6 {
                        return Err(ContentError(format!("{}: height must be a range within 1..=6", at())));
                    }
                    share("growth", *growth)?;
                    plants(list)?;
                }
                Shape::StandingStones { radius, count, height, stones } => {
                    range("radius", *radius, 7.0)?;
                    if count.0 < 3 || count.0 > count.1 || count.1 > 16 || height.0 < 1 || height.0 > height.1 || height.1 > 6 {
                        return Err(ContentError(format!("{}: 3..=16 stones, 1..=6 tall", at())));
                    }
                    material(stones)?;
                }
                Shape::FairyRing { radius, plants: list } => {
                    range("radius", *radius, 7.0)?;
                    plants(list)?;
                }
                Shape::Scatter { count, spread, block: name } => {
                    if count.0 < 1 || count.0 > count.1 || count.1 > 12 {
                        return Err(ContentError(format!("{}: count must be a range within 1..=12", at())));
                    }
                    range("spread", (*spread, *spread), 6.0)?;
                    block(&at, name)?;
                }
                Shape::Litter { top, cover } => {
                    material(top)?;
                    share("cover", *cover)?;
                }
                Shape::LeafPiles { cover } => share("cover", *cover)?,
                Shape::Foot { plants: list, cover } => {
                    share("cover", *cover)?;
                    plants(list)?;
                }
            }
            if def.ornament() {
                share("chance", def.chance.unwrap_or(1.0))?;
                if def.trees.is_empty() {
                    return Err(ContentError(format!("{}: an ornament needs trees to decorate", at())));
                }
                for select in &def.trees {
                    positive(&at, "weight", select.weight())?;
                    if let Select::Name(name, _) = select
                        && !self.trees.iter().any(|t| t.name == *name)
                    {
                        return Err(ContentError(format!("{}: unknown tree `{name}`", at())));
                    }
                }
            } else {
                let spacing = def.spacing.ok_or_else(|| ContentError(format!("{}: needs a spacing", at())))?;
                if !(spacing >= 2.0 && spacing.is_finite()) {
                    return Err(ContentError(format!("{}: spacing must be at least 2 blocks", at())));
                }
            }
            let reach = def.reach();
            if reach > crate::features::REACH as f32 {
                return Err(ContentError(format!("{}: reaches {reach} blocks, more than {}", at(), crate::features::REACH)));
            }
            if def.grounds().is_empty() {
                return Err(ContentError(format!("{}: stands on no ground", at())));
            }
            for follow in &def.follows {
                if !(follow.1.is_finite() && follow.2.is_finite() && follow.1 != follow.2) {
                    return Err(ContentError(format!("{}: a ramp needs two different, finite ends", at())));
                }
                if let Field::Noise(noise) = &follow.0
                    && !(noise.scale > 0.0 && (1..=6).contains(&noise.octaves))
                {
                    return Err(ContentError(format!("{}: noise `{}` needs a positive scale and 1..=6 octaves", at(), noise.name)));
                }
            }
            for (what, range) in [("temperature", def.comfort.temperature), ("moisture", def.comfort.moisture)] {
                if let Some((lo, hi)) = range
                    && !(lo.is_finite() && hi.is_finite() && lo < hi)
                {
                    return Err(ContentError(format!("{}: the {what} comfort range ({lo}, {hi}) is empty", at())));
                }
            }
        }
        Ok(())
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

    fn check_decoration(
        &self,
        decoration: &DecorationDef,
        strata: bool,
        registry: &BlockRegistry,
        at: &dyn Fn() -> String,
        block: &BlockCheck<'_, str>,
        blocks: &BlockCheck<'_, [Select]>,
    ) -> Result<(), ContentError> {
        let material = |material: &Material| match material {
            Material::Block(name) => block(at, name),
            Material::Mix(selects) => blocks(at, selects),
            Material::Stones => Ok(()),
            Material::Strata if strata => Ok(()),
            Material::Strata => Err(ContentError(format!("{}: uses strata but the biome has none", at()))),
        };
        material(&decoration.block)?;
        if let Some((tip, chance)) = &decoration.tip {
            material(tip)?;
            if !(0.0..=1.0).contains(chance) {
                return Err(ContentError(format!("{}: the tip's chance must be in 0..=1", at())));
            }
        }
        if !(0.0..=1.0).contains(&decoration.chance) {
            return Err(ContentError(format!("{}: a chance must be in 0..=1", at())));
        }
        let (lo, hi) = decoration.length;
        if !(1 <= lo && lo <= hi && hi <= 32) {
            return Err(ContentError(format!("{}: the length ({lo}, {hi}) must be within 1..=32 and increase", at())));
        }
        for tag in &decoration.on_tags {
            if registry.tagged(tag).next().is_none() {
                return Err(ContentError(format!("{}: no block has the tag `{tag}`", at())));
            }
        }
        decoration.when.iter().try_for_each(|c| self.check_condition(c, at))
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
            | Condition::River(a, b)
            | Condition::Clearing(a, b)
            | Condition::Stand(a, b) => {
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

fn qualify_match(package: &str, m: &BlockMatch) -> BlockMatch {
    match m {
        BlockMatch::Name(name) => BlockMatch::Name(qualify(package, name)),
        BlockMatch::Tag(tag) => BlockMatch::Tag(tag.clone()),
    }
}

fn qualify_decoration(package: &str, def: GroundDecorationDef) -> GroundDecorationDef {
    let q = |name: &str| qualify(package, name);
    let selects = |list: Vec<Select>| list.iter().map(|s| qualify_select(package, s)).collect();
    let shape = match def.shape {
        Shape::Patch { radius, top } => Shape::Patch { radius, top: qualify_material(package, top) },
        Shape::Boulder { radius, stones, moss } => Shape::Boulder { radius, stones: qualify_material(package, stones), moss },
        Shape::Rocks { count, spread, size, stones, moss } => Shape::Rocks { count, spread, size, stones: qualify_material(package, stones), moss },
        Shape::Bush { radius, leaves, log } => Shape::Bush { radius, leaves: leaves.as_deref().map(q), log: log.as_deref().map(q) },
        Shape::FallenLog { length, thick, hollow, branches, stump, growth, plants } => {
            Shape::FallenLog { length, thick, hollow, branches, stump, growth, plants: selects(plants) }
        }
        Shape::Stump { height, growth, plants } => Shape::Stump { height, growth, plants: selects(plants) },
        Shape::StandingStones { radius, count, height, stones } => Shape::StandingStones { radius, count, height, stones: qualify_material(package, stones) },
        Shape::FairyRing { radius, plants } => Shape::FairyRing { radius, plants: selects(plants) },
        Shape::Scatter { count, spread, block } => Shape::Scatter { count, spread, block: q(&block) },
        Shape::Litter { top, cover } => Shape::Litter { top: qualify_material(package, top), cover },
        Shape::LeafPiles { cover } => Shape::LeafPiles { cover },
        Shape::Foot { plants, cover } => Shape::Foot { plants: selects(plants), cover },
    };
    GroundDecorationDef { name: q(&def.name), shape, trees: selects(def.trees), ..def }
}

fn qualify_layers(package: &str, layers: LayersDef) -> LayersDef {
    LayersDef {
        top: qualify_material(package, layers.top),
        filler: layers.filler.map(|m| qualify_material(package, m)),
        depth: layers.depth,
    }
}

fn qualify_surface(package: &str, surface: SurfaceDef) -> SurfaceDef {
    SurfaceDef {
        rules: surface.rules.into_iter().map(|r| RuleDef { when: r.when, place: qualify_layers(package, r.place) }).collect(),
        include: surface.include.iter().map(|n| qualify(package, n)).collect(),
        ground: qualify_layers(package, surface.ground),
        underwater: qualify_layers(package, surface.underwater),
        shore: surface.shore.map(|l| qualify_layers(package, l)),
        cliff: surface.cliff.map(|m| qualify_material(package, m)),
    }
}


fn qualify_biome(package: &str, biome: RawBiome) -> RawBiome {
    let q = |name: &str| qualify(package, name);
    RawBiome {
        name: q(&biome.name),
        inherit: biome.inherit.as_deref().map(q),
        ecotone: biome.ecotone.map(|e| EcotoneDef { between: (q(&e.between.0), q(&e.between.1)), ..e }),
        surface: biome.surface.map(|s| qualify_surface(package, s)),
        strata: biome.strata.map(|names| names.iter().map(|n| q(n)).collect()),
        atmosphere: biome.atmosphere.map(|a| AtmosphereDef {
            particles: a.particles.map(|p| p.iter().map(|s| qualify_select(package, s)).collect()),
            ..a
        }),
        flora: biome.flora.map(|f| FloraDef {
            trees: f.trees.iter().map(|s| qualify_select(package, s)).collect(),
            plants: f.plants.iter().map(|s| qualify_select(package, s)).collect(),
            wonders: f.wonders.map(|w| w.iter().map(|s| qualify_select(package, s)).collect()),
            decorations: f.decorations.map(|d| d.iter().map(|s| qualify_select(package, s)).collect()),
            ..f
        }),
        stones: biome.stones.map(|s| match s {
            Stones::Province => Stones::Province,
            Stones::Blocks(selects) => Stones::Blocks(selects.iter().map(|s| qualify_select(package, s)).collect()),
        }),
        field_stones: biome.field_stones.map(|v| v.iter().map(|s| qualify_select(package, s)).collect()),
        variants: biome.variants.map(|v| v.into_iter().map(|(name, share)| (q(&name), share)).collect()),
        cave_decoration: biome.cave_decoration.map(|list| {
            list.into_iter()
                .map(|d| DecorationDef {
                    block: qualify_material(package, d.block),
                    tip: d.tip.map(|(m, chance)| (qualify_material(package, m), chance)),
                    ..d
                })
                .collect()
        }),
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
                level: field!(level).unwrap_or(1),
                surface,
                stones: field!(stones).unwrap_or_default(),
                field_stones: field!(field_stones).unwrap_or_default(),
                strata: field!(strata).unwrap_or_default(),
                colors: field!(colors).unwrap_or_default(),
                flora: field!(flora).unwrap_or_default(),
                atmosphere: field!(atmosphere).unwrap_or_default(),
                variants: biome.variants.clone().unwrap_or_default(),
                cave_decoration: field!(cave_decoration).unwrap_or_default(),
            })
        })
        .collect()
}

fn tenth() -> f32 {
    0.1
}

fn soil() -> Vec<Ground> {
    vec![Ground::Soil]
}

fn one() -> f32 {
    1.0
}

fn one_octave() -> i32 {
    1
}

fn single() -> (i32, i32) {
    (1, 1)
}

fn temperature_axis() -> Axis {
    Axis::Temperature
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> BlockRegistry {
        crate::test_registry()
    }

    fn base_files() -> Vec<ContentFile> {
        BASE_FILES.iter().map(|(name, source)| ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() }).collect()
    }

    /// The base package with `biomes.ron` replaced by `source`, and an extra package.
    fn load_with(biomes: &str) -> Result<WorldContent, ContentError> {
        let mut files = base_files();
        with_biomes(&mut files, biomes);
        WorldContent::from_files(files.iter(), &registry())
    }

    /// Replaces the base `biomes.ron` with `source`, and empties `lairs.ron`, whose lairs
    /// lie under base biomes `source` may not have.
    fn with_biomes(files: &mut [ContentFile], source: &str) {
        files.iter_mut().find(|f| f.name == "biomes.ron").unwrap().source = source.into();
        files.iter_mut().find(|f| f.name == "lairs.ron").unwrap().source = "(lairs: [])".into();
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

    /// A wood is found only where its tree grows: a biome grows a tree built of a wood
    /// named after a tree of another family (lemons of jungle wood, olives of acacia)
    /// only where a tree of the wood's own family grows too, so acacia logs never turn up
    /// in taiga (dead snags there are `dead_log`, which breaks to sticks) and jungle
    /// timber never in a laurel forest. Common woods build any tree anywhere, and every
    /// wood a tree is built of is common or has a family (`woods` in trees.ron).
    #[test]
    fn woods_grow_only_where_their_trees_do() {
        let content = WorldContent::base(&registry()).unwrap();
        let woods = &content.woods;
        let picks = |select: &Select, tree: &TreeDef| match select {
            Select::Name(name, _) => *name == tree.name,
            Select::Tag(tag, _) => tree.tags.contains(tag),
        };
        let mut failures = Vec::new();
        for tree in &content.trees {
            for wood in [Some(&tree.log), tree.root_block.as_ref()].into_iter().flatten() {
                if !woods.common.contains(wood) && !woods.families.contains_key(wood) {
                    failures.push(format!("{} is built of {wood}, which is neither common nor of a family", tree.name));
                }
            }
        }
        for biome in &content.biomes {
            let grows: Vec<&TreeDef> = content
                .trees
                .iter()
                .filter(|t| biome.flora.trees.iter().any(|s| picks(s, t)) && !t.tags.iter().any(|tag| biome.flora.exclude.contains(tag)))
                .collect();
            for tree in &grows {
                for wood in [Some(&tree.log), tree.root_block.as_ref()].into_iter().flatten() {
                    let Some(family) = woods.families.get(wood) else { continue };
                    if !family.contains(&tree.name) && !grows.iter().any(|t| family.contains(&t.name)) {
                        failures.push(format!("{} grows {} of {wood}, with none of {family:?}", biome.name, tree.name));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{} woods out of place:\n{}", failures.len(), failures.join("\n"));
        let dead = content.trees.iter().find(|t| t.name == "base:dead_tree").expect("dead_tree");
        assert_eq!(dead.log, "base:dead_log");
        let registry = registry();
        let log = registry.def(registry.expect("base:dead_log"));
        assert_eq!(log.drops.as_deref(), Some("core:air"), "a dead log is never an item (it breaks to a stick: tools.ron)");
    }

    /// Every base biome, on the surface and underground, sets its own level (P15), as one
    /// left out would quietly take its parent's or 1: the gentle land players start in is
    /// level 1 and the fantastical lands lie high. A level outside 1 ..= 10 is refused.
    #[test]
    fn every_base_biome_has_its_level() {
        for (name, source) in BASE_FILES.iter().filter(|(name, _)| name.ends_with("biomes.ron")) {
            let file: BiomeFile = parse(name, source).unwrap();
            for biome in &file.biomes {
                assert!(biome.level.is_some(), "{name}: biome `{}` has no level", biome.name);
            }
        }
        let content = WorldContent::base(&registry()).unwrap();
        let level = |name: &str| content.biomes.iter().find(|b| b.name == format!("base:{name}")).unwrap().level;
        for gentle in ["plains", "meadow", "forest", "river", "cave"] {
            assert_eq!(level(gentle), 1, "{gentle}");
        }
        for biome in content.biomes.iter().filter(|b| b.tags.iter().any(|t| t == "fantastical" || t == "fantasy")) {
            assert!(biome.level >= 7, "{} is fantastical at level {}", biome.name, biome.level);
        }
        let error = load_with(&biomes(&format!("(name: \"harsh\", level: 11, climate: [()], {GROUND})"))).unwrap_err();
        assert!(error.0.contains("level 11"), "{error}");
    }

    /// The snow blanket spares what its rules say, and refuses tags no block has and
    /// entries it cannot read.
    #[test]
    fn the_snow_blanket_spares_built_stone_and_checks_its_rules() {
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let blanket = &content.climate.frost.blanket;
        let snowed = |name: &str| blanket.iter().any(|rule| rule.applies(registry.def(registry.expect(name))));
        assert!(snowed("base:stone") && snowed("base:granite") && snowed("base:moss_block"));
        for built in ["base:cobblestone", "base:stone_bricks", "base:polished_stone", "base:mossy_cobblestone", "base:cobbled_granite"] {
            assert!(!snowed(built), "{built} turns to snow");
        }
        let with_blanket = |entries: &str| {
            let mut files = base_files();
            let climate = files.iter_mut().find(|f| f.name == "climate.ron").unwrap();
            let start = climate.source.find("blanket: [").unwrap();
            let end = start + climate.source[start..].find("],\n").unwrap() + 2;
            climate.source.replace_range(start..end, &format!("blanket: [{entries}],"));
            WorldContent::from_files(files.iter(), &registry)
        };
        assert!(with_blanket(r#"(covers: Tag("stone"), to: "snow_block", not: ["building"])"#).is_ok());
        let spares = with_blanket(r#"(covers: Tag("stone"), to: "snow_block", not: ["no_such_tag"])"#).unwrap_err().0;
        assert!(spares.contains("no_such_tag"), "{spares}");
        assert!(with_blanket(r#"(covers: Tag("no_such_tag"), to: "snow_block")"#).is_err());
        assert!(with_blanket(r#"(Tag("stone"), "snow_block")"#).is_err(), "the old form is refused, not misread");
        assert!(with_blanket(r#"(covers: Tag("stone"), to: "snow_block", except: ["building"])"#).is_err());
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
            ("(name: \"a\", climate: [()], surface: (rules: [(when: [Province(\"pumice\")], place: (top: Stones))], ground: (top: Stones), underwater: (top: Stones)))", "rock province"),
            ("(name: \"a\", climate: [()], surface: (ground: (top: Mix([Tag(\"no_such_tag\", 1.0)])), underwater: (top: Stones)))", "tag `no_such_tag`"),
            ("(name: \"a\", climate: [()], flora: (trees: [Name(\"baobob\", 1.0)]), SURFACE)", "unknown tree `base:baobob`"),
            ("(name: \"a\", climate: [()], flora: (forest: 1.5), SURFACE)", "forest must be in 0..=1"),
        ];
        for (extra, expected) in cases {
            let message = error(&extra.replace("SURFACE", GROUND));
            assert!(message.contains(expected), "expected `{expected}` in: {message}");
            assert!(message.contains("biomes.ron") || expected == "inherits from itself", "no file in: {message}");
        }
    }

    /// A later package may class a wood the other way, common or with a family, and it
    /// is classed its way (with a note in the log); family members join the family.
    #[test]
    fn a_later_package_reclasses_a_wood() {
        let registry = registry();
        let mut files = base_files();
        let source = "(woods: (common: [\"base:acacia_log\"], families: {\"base:oak_log\": [\"base:oak\"], \"base:cherry_log\": [\"base:wild_cherry\"]}), trees: [])";
        files.push(ContentFile { package: "mod".into(), name: "trees.ron".into(), source: source.into() });
        let content = WorldContent::from_files(files.iter(), &registry).unwrap();
        assert!(content.notes.iter().any(|n| n == "mod/trees.ron makes base:acacia_log a common wood, replacing its family [\"base:acacia\"]"), "{:?}", content.notes);
        let woods = content.woods;
        assert!(woods.common.contains(&"base:acacia_log".to_string()) && !woods.families.contains_key("base:acacia_log"));
        assert!(!woods.common.contains(&"base:oak_log".to_string()) && woods.families["base:oak_log"] == ["base:oak"]);
        let cherry = &woods.families["base:cherry_log"];
        assert!(cherry.contains(&"base:cherry".to_string()) && cherry.iter().filter(|t| *t == "base:wild_cherry").count() == 1, "members join the family once: {cherry:?}");
    }

    /// A later package sets single-valued data field by field (Design draft 91): what its
    /// file names replaces base's (a noise or field of the land by name, in place, new
    /// ones after), everything else keeps base's values, and loading notes what it
    /// replaced.
    #[test]
    fn a_later_package_sets_climate_land_and_trails_field_by_field() {
        let registry = registry();
        let base = WorldContent::base(&registry).unwrap();
        let mut files = base_files();
        for (name, source) in [
            ("climate.ron", "(scales: (aether: 3000.0), frost: (ice: \"base:packed_ice\"), levels: (homes: (very_rare: 8)))"),
            ("land.ron", "(vertical_scale: 1.25, density: (squash: 2.0), noises: [(\"hills\", (wavelength: 700.0, octaves: [1.0])), (\"dunes\", (wavelength: 90.0, octaves: [1.0]))])"),
            ("trails.ron", "(edge: \"base:coarse_dirt\")"),
            ("trees.ron", "(density: 0.5, trees: [])"),
        ] {
            files.push(ContentFile { package: "mod".into(), name: name.into(), source: source.into() });
        }
        let content = WorldContent::from_files(files.iter(), &registry).unwrap();
        let same = |a: &dyn std::fmt::Debug, b: &dyn std::fmt::Debug| format!("{a:?}") == format!("{b:?}");
        // What the mod set.
        let (climate, land, trails) = (&content.climate, &content.land, &content.trails);
        assert_eq!((climate.scales.aether, climate.frost.ice.as_str()), (3000.0, "base:packed_ice"));
        assert_eq!(climate.levels.homes.very_rare, 8);
        assert_eq!(climate.levels, crate::levels::LevelsDef { homes: crate::levels::Homes { very_rare: 8, ..base.climate.levels.homes }, ..base.climate.levels.clone() }, "the rest of the levels is base's");
        assert_eq!((land.vertical_scale, land.density.squash, trails.edge.as_str()), (1.25, 2.0, "base:coarse_dirt"));
        let hills = land.noises.iter().position(|(n, _)| n == "hills").unwrap();
        assert_eq!(hills, base.land.noises.iter().position(|(n, _)| n == "hills").unwrap(), "a named entry is replaced in place");
        assert_eq!(land.noises[hills].1.wavelength, 700.0);
        assert_eq!(land.noises.last().unwrap().0, "dunes", "a new one comes after");
        assert_eq!(content.tree_density, 0.5);
        // Everything else is base's.
        let base_scales = ScalesDef { aether: 3000.0, ..base.climate.scales.clone() };
        assert!(same(&climate.scales, &base_scales) && same(&climate.humidity, &base.climate.humidity));
        assert!(same(&climate.temperature, &base.climate.temperature) && same(&climate.remaps, &base.climate.remaps) && climate.zones == base.climate.zones);
        assert!(same(&FrostDef { ice: base.climate.frost.ice.clone(), ..climate.frost.clone() }, &base.climate.frost));
        assert!(same(&DensityDef { squash: base.land.density.squash, ..land.density.clone() }, &base.land.density));
        assert!(same(&land.fields, &base.land.fields) && same(&land.warps, &base.land.warps) && land.ceiling == base.land.ceiling);
        assert_eq!(land.noises.len(), base.land.noises.len() + 1);
        assert!(same(&land.noises.iter().filter(|(n, _)| n != "hills" && n != "dunes").collect::<Vec<_>>(), &base.land.noises.iter().filter(|(n, _)| n != "hills").collect::<Vec<_>>()));
        assert_eq!(*trails, TrailsDef { edge: "base:coarse_dirt".into(), ..base.trails.clone() });
        // And the notes say so.
        for note in [
            "mod/climate.ron replaces the climate.ron base set (scales.aether, frost.ice, levels.homes.very_rare)".to_owned(),
            "mod/land.ron replaces the land.ron base set (vertical_scale, density.squash, noises.hills)".to_owned(),
            "mod/land.ron adds to the land.ron (noises.dunes)".to_owned(),
            "mod/trails.ron replaces the trails.ron base set (edge)".to_owned(),
            format!("mod/trees.ron replaces the tree density base set ({} to 0.5)", base.tree_density),
        ] {
            assert!(content.notes.iter().any(|n| *n == note), "no note `{note}` in {:?}", content.notes);
        }
        assert!(base.notes.is_empty(), "base alone replaces nothing: {:?}", base.notes);
    }

    /// With three packages a note names the package whose value each field replaces, field
    /// by field (so one note a package replaced), says what is new, and a later file that
    /// changes nothing leaves no note.
    #[test]
    fn people_load_notes_follow_each_field_and_routine() {
        let registry = registry();
        let mut files = base_files();
        for (package, source) in [
            ("first", "(people: (jitter: 0.02, live: (search: 4000), routines: [(name: \"worker\", slots: [(until: 1.0, go: [(place: Home)])])] ))"),
            ("second", "(people: (jitter: 0.01, live: (search: 3000)))"),
            ("empty", "()"),
        ] {
            files.push(ContentFile { package: package.into(), name: "settlements.ron".into(), source: source.into() });
        }
        let content = WorldContent::from_files(files.iter(), &registry).unwrap();
        assert_eq!(content.settlements.people.jitter, 0.01);
        assert_eq!(content.settlements.people.live.search, 3000);
        for note in [
            "first/settlements.ron replaces the settlements.ron base set (people.jitter)",
            "first/settlements.ron replaces the settlements.ron base set (people.live.search)",
            "first/settlements.ron replaces the settlements.ron base set (people.routines.worker)",
            "second/settlements.ron replaces the settlements.ron first set (people.jitter)",
            "second/settlements.ron replaces the settlements.ron first set (people.live.search)",
        ] {
            assert!(content.notes.iter().any(|n| n == note), "missing {note}: {:?}", content.notes);
        }
        assert!(!content.notes.iter().any(|n| n.contains("empty/")));
    }

    #[test]
    fn notes_name_the_package_each_field_replaces() {
        let registry = registry();
        let mut files = base_files();
        for (package, name, source) in [
            ("first", "climate.ron", "(scales: (aether: 3000.0))"),
            ("second", "climate.ron", "(scales: (aether: 2500.0, wetness: 1.5))"),
            ("second", "land.ron", "(noises: [(\"dunes\", (wavelength: 90.0, octaves: [1.0]))])"),
            ("third", "land.ron", "(noises: [(\"dunes\", (wavelength: 80.0, octaves: [1.0]))])"),
            ("third", "trails.ron", "()"),
        ] {
            files.push(ContentFile { package: package.into(), name: name.into(), source: source.into() });
        }
        let content = WorldContent::from_files(files.iter(), &registry).unwrap();
        assert_eq!((content.climate.scales.aether, content.climate.scales.wetness), (2500.0, 1.5));
        for note in [
            "first/climate.ron replaces the climate.ron base set (scales.aether)",
            "second/climate.ron replaces the climate.ron base set (scales.wetness)",
            "second/climate.ron replaces the climate.ron first set (scales.aether)",
            "second/land.ron adds to the land.ron (noises.dunes)",
            "third/land.ron replaces the land.ron second set (noises.dunes)",
        ] {
            assert!(content.notes.iter().any(|n| n == note), "no note `{note}` in {:?}", content.notes);
        }
        assert!(!content.notes.iter().any(|n| n.contains("third/trails.ron")), "an empty file leaves no note: {:?}", content.notes);
    }

    #[test]
    fn broken_trees_and_plants_are_refused() {
        let registry = registry();
        let load = |name: &str, source: &str| {
            let mut files = base_files();
            files.push(ContentFile { package: "mod".into(), name: name.into(), source: source.into() });
            WorldContent::from_files(files.iter(), &registry).unwrap_err().0
        };
        let tree = |rest: &str| format!("(trees: [(name: \"t\", log: \"base:oak_log\", height: (4, 6), {rest})])");
        let cases = [
            ("trees.ron", tree("crown: Bush(radius: (1.0, 2.0))"), "needs leaves"),
            ("trees.ron", tree("leaves: \"base:oak_leaf\", crown: Bush(radius: (1.0, 2.0))"), "unknown block `base:oak_leaf`"),
            ("trees.ron", tree("leaves: \"base:oak_leaves\", lean: 3.0, crown: Palm(fronds: (2, 3), length: (6.0, 7.0))"), "reaches 10"),
            ("trees.ron", tree("crown: Bare, comfort: (temperature: (20.0, 10.0))"), "comfort range"),
            ("trees.ron", tree("crown: Bare, colour: 1"), "colour"),
            ("land.ron", "(vertical_scal: 1.0)".into(), "mod/land.ron:"),
            ("land.ron", "(vertical_scal: 1.0)".into(), "vertical_scal"),
            ("climate.ron", "(scales: (aether: \"far\"))".into(), "mod/climate.ron:"),
            ("climate.ron", "(scales: (aether_: 1.0))".into(), "mod/climate.ron:"),
            ("climate.ron", "(scales: (aether_: 1.0))".into(), "aether_"),
            ("climate.ron", "(levels: (homes: (rare: 6)))".into(), "strictly decreasing"),
            ("trees.ron", "(woods: (common: [\"base:oak_lgo\"]), trees: [])".into(), "unknown block `base:oak_lgo`"),
            ("trees.ron", "(woods: (families: {\"base:acacia_log\": [\"baobob\"]}), trees: [])".into(), "no tree is named `mod:baobob`"),
            ("trees.ron", "(woods: (common: [\"base:acacia_log\"], families: {\"base:acacia_log\": [\"base:acacia\"]}), trees: [])".into(), "both as a common wood and with a family"),
            ("trees.ron", "(woods: (families: {\"base:acacia_log\": [\"base:oak\"]}), trees: [])".into(), "`base:oak` is not built of `base:acacia_log`"),
            ("plants.ron", "(plants: [(name: \"p\", block: \"base:poppy\", top: \"base:poppy\", stack: (1, 2))])".into(), "a stack"),
        ];
        for (file, source, expected) in cases {
            let message = load(file, &source);
            assert!(message.contains(expected), "expected `{expected}` in: {message}");
        }
    }

    /// A rare herb's patches are refused at load unless they make sense: a rate above 0, 1
    /// to 25 plants a patch, an aim, `named_only`, and no biome naming it so heavily that a
    /// cell would need more than one patch.
    #[test]
    fn bad_patches_are_refused() {
        let registry = registry();
        let load = |patches: &str, weight: f32| {
            let mut files = base_files();
            with_biomes(&mut files, &biomes(&format!("(name: \"herbs\", climate: [()], flora: (plants: [Name(\"mod:herb\", {weight:?})]), {GROUND})")));
            let plant = format!("(plants: [(name: \"herb\", block: \"base:poppy\", {patches})])");
            files.push(ContentFile { package: "mod".into(), name: "plants.ron".into(), source: plant });
            WorldContent::from_files(files.iter(), &registry).map(|_| ()).map_err(|e| e.0)
        };
        assert_eq!(load("named_only: true, patches: (rate: 200.0, size: (3, 6), aim: 200.0)", 1.0), Ok(()));
        let cases = [
            ("named_only: true, patches: (rate: 0.0, size: (3, 6), aim: 200.0)", 1.0, "patch rate 0"),
            ("named_only: true, patches: (rate: inf, size: (3, 6), aim: 200.0)", 1.0, "patch rate inf"),
            ("named_only: true, patches: (rate: 200.0, size: (0, 6), aim: 200.0)", 1.0, "1 to 25 plants"),
            ("named_only: true, patches: (rate: 200.0, size: (6, 3), aim: 200.0)", 1.0, "1 to 25 plants"),
            ("named_only: true, patches: (rate: 200.0, size: (3, 26), aim: 200.0)", 1.0, "1 to 25 plants"),
            ("named_only: true, patches: (rate: 200.0, size: (3, 6), aim: -1.0)", 1.0, "patch aim"),
            ("patches: (rate: 200.0, size: (3, 6), aim: 200.0)", 1.0, "named_only"),
            // 200 plants a million blocks at weight 30, in patches of 4.5 on average: 1.37
            // patches a 32-block cell.
            ("named_only: true, patches: (rate: 200.0, size: (3, 6), aim: 200.0)", 30.0, "plant `mod:herb` at weight 30"),
        ];
        for (patches, weight, expected) in cases {
            let message = load(patches, weight).unwrap_err();
            assert!(message.contains(expected), "expected `{expected}` in: {message}");
            assert!(message.contains("mod:herb") || message.contains("`herb`"), "no plant named in: {message}");
        }
    }

    /// Single-valued tarn settings inherit field by field, including nested country
    /// settings. Notes must name the last setter of the field, rather than the last file.
    #[test]
    fn tarn_packages_inherit_each_field() {
        let registry = registry();
        let base = WorldContent::base(&registry).unwrap().tarns.unwrap();
        let mut files = base_files();
        for (package, settings) in [("tune", "chance: 0.4, size: (9, 24)"), ("country", "country: (share: 0.6)")] {
            files.push(ContentFile { package: package.into(), name: "terrain.ron".into(), source: format!("#![enable(implicit_some)]\n(shapes: [], tarns: ({settings}))") });
        }
        let content = WorldContent::from_files(files.iter(), &registry).unwrap();
        let mut expected = base;
        expected.chance = 0.4;
        expected.size = (9, 24);
        expected.country.share = 0.6;
        assert_eq!(content.tarns.unwrap(), expected);
        assert!(content.notes.contains(&"tune/terrain.ron replaces the terrain.ron base set (tarns.size, tarns.chance)".into()));
        assert!(content.notes.contains(&"country/terrain.ron replaces the terrain.ron base set (tarns.country.share)".into()));
        files.retain(|f| f.name != "terrain.ron" || f.package != "base");
        let error = WorldContent::from_files(files.iter(), &registry).unwrap_err().0;
        assert!(error.contains("tune/terrain.ron") && error.contains("missing field"), "a partial first declaration was accepted: {error}");
    }

    /// Tarn data is refused at load unless it makes sense, naming what is wrong.
    #[test]
    fn bad_tarns_are_refused() {
        let registry = registry();
        let load = |tarns: &str| {
            let mut files = base_files();
            files.push(ContentFile { package: "mod".into(), name: "terrain.ron".into(), source: format!("#![enable(implicit_some)]\n(shapes: [], tarns: ({tarns}))") });
            WorldContent::from_files(files.iter(), &registry).map(|c| {
                assert!(c.notes.iter().any(|n| n == "mod/terrain.ron replaces the terrain.ron base set (tarns.climate, tarns.floor, tarns.depth, tarns.size, tarns.chance, tarns.country.scale, tarns.country.share)"));
                c.tarns.unwrap().chance
            }).map_err(|e| e.0)
        };
        let good = "climate: [(temperature: (\"frozen\", \"cool\"))], floor: 56.0, depth: 2.0, size: (8, 25), chance: 0.3, country: (scale: 1000.0, share: 0.5)";
        assert_eq!(load(good), Ok(0.3), "a later package sets every tarn field");
        for (from, to, expected) in [
            ("chance: 0.3", "chance: 1.5", "chance 1.5"),
            ("floor: 56.0", "floor: -3.0", "floor -3"),
            ("depth: 2.0", "depth: 0.0", "depth 0"),
            ("size: (8, 25)", "size: (26, 25)", "size (26, 25)"),
            ("size: (8, 25)", "size: (0, 25)", "size (0, 25)"),
            ("share: 0.5", "share: 2.0", "country share 2"),
            ("\"cool\"", "\"balmy\"", "unknown Temperature band `balmy`"),
        ] {
            let message = load(&good.replace(from, to)).unwrap_err();
            assert!(message.contains("terrain.ron: tarns") && message.contains(expected), "expected `{expected}` in: {message}");
        }
    }

    #[test]
    fn holes_in_the_biome_table_are_refused() {
        let only_cold = format!("#![enable(implicit_some)]\n(biomes: [(name: \"cold\", climate: [(temperature: (\"frozen\", \"cold\"))], {GROUND})])");
        let message = load_with(&only_cold).unwrap_err().0;
        assert!(message.contains("holes") && message.contains("cool"), "{message}");
    }

    #[test]
    fn broken_cave_biomes_fail_with_clear_errors() {
        let load = |caves: &str| {
            let mut files = base_files();
            let source = format!("#![enable(implicit_some)]\n(biomes: [{caves}])");
            files.iter_mut().find(|f| f.name == "cave_biomes.ron").unwrap().source = source;
            WorldContent::from_files(files.iter(), &registry())
        };
        let cave = |extra: &str| format!("(name: \"c\", layer: Underground, climate: [(depth: (4.0, 80.0))], {GROUND}, {extra})");
        assert!(load(&cave("cave_decoration: [(on: Ceiling, chance: 0.1, block: Block(\"ice\"), length: (1, 3))]")).is_ok());
        let cases = [
            (cave("cave_decoration: [(on: Ceiling, chance: 1.5, block: Block(\"ice\"))]"), "chance"),
            (cave("cave_decoration: [(on: Floor, chance: 0.1, block: Block(\"ice\"), length: (3, 1))]"), "length"),
            (cave("cave_decoration: [(on: Floor, chance: 0.1, block: Block(\"icee\"))]"), "unknown block `base:icee`"),
            (cave("cave_decoration: [(on: Floor, chance: 0.1, on_tags: [\"nope\"], block: Block(\"ice\"))]"), "tag `nope`"),
            (cave("cave_decoration: [(on: Floor, chance: 0.1, block: Strata)]"), "strata"),
            ("(name: \"c\", layer: Underground, climate: [(depth: (80.0, 4.0))], SURFACE)".replace("SURFACE", GROUND), "depth range"),
            ("(name: \"c\", layer: Underground, climate: [(provinces: [\"pumice\"])], SURFACE)".replace("SURFACE", GROUND), "rock province"),
            ("(name: \"c\", layer: Underground, SURFACE)".replace("SURFACE", GROUND), "climate box"),
            // Underground parts on a surface biome.
            ("(name: \"c\", climate: [(depth: (4.0, 80.0))], SURFACE)".replace("SURFACE", GROUND), "underground biomes"),
        ];
        for (source, expected) in cases {
            let message = load(&source).unwrap_err().0;
            assert!(message.contains(expected) && message.contains("cave_biomes.ron"), "expected `{expected}` in: {message}");
        }
    }

    /// A biome's field stones read as two stones, not one (Review on decor): where it
    /// names two or more, the lightest and darkest differ in brightness by at least 30 of
    /// 255. Stone and andesite, 10 apart, looked like one stone at 3840×2160.
    #[test]
    fn field_stones_differ_enough_to_show() {
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let luma = |name: &str| {
            let [r, g, b] = registry.def(registry.expect(name)).color;
            0.3 * r as f32 + 0.59 * g as f32 + 0.11 * b as f32
        };
        let mut checked = 0;
        for biome in &content.biomes {
            let lumas: Vec<f32> = biome.field_stones.iter().filter_map(|s| if let Select::Name(name, _) = s { Some(luma(name)) } else { None }).collect();
            if lumas.len() < 2 {
                continue;
            }
            checked += 1;
            let spread = lumas.iter().copied().fold(f32::MIN, f32::max) - lumas.iter().copied().fold(f32::MAX, f32::min);
            assert!(spread >= 30.0, "{}: its field stones {:?} differ by only {spread:.0} in brightness", biome.name, biome.field_stones);
        }
        assert!(checked >= 5, "{checked} biomes with field stones");
    }
}
