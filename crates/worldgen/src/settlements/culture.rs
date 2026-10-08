//! Cultures: who built a settlement, drawn from `cultures.ron` (draft 108 of the design
//! wiki, P57a).
//!
//! The world is cut into regions about `rules.field.cell` blocks across (Voronoi cells on a
//! jittered grid), each naming one family of cultures: the one most of the cell's sample
//! points suit by biome tag and temperature, weighed by the family's weight and a little
//! luck. So neighbours share a family; and within `rules.field.border` blocks of a cell's
//! edge a settlement also takes a share of the next cell's family, so borders mix. Two
//! families claim by site instead: river folk where fresh water and trade run high, and
//! the Old Empire in cities (P57c).
//!
//! A family is a bundle of leanings: eight axes ([`Axis`], each 0 ..= 1), weights for the
//! styles, houses, roofs, props, centrepieces, layouts and extras it favours or shuns (a
//! weight of 0 removes an entry), a palette of eight colours (blocks), the name parts of
//! its places, and how much it favours each quirk. The lists in `settlements.ron` still
//! hold every entry; the family only multiplies their weights.
//!
//! No two places of a family are alike. A settlement's axes are the family's mean, a
//! regional drift, local noise and shifts from its site; its family entries are mutated
//! (one to three favourites dropped, nought to two borrowed from the second family or a
//! random one, three to five palette colours kept) and it has a quirk, now and then two.
//!
//! Like the rest of planning this is a pure function of the seed and the place: nothing
//! here depends on which square, column or thread came first.

use std::collections::{BTreeMap, BTreeSet};

use rustc_hash::FxHashSet;
use serde::Deserialize;
use voxmar_engine::block::BlockRegistry;
use voxmar_engine::glam::IVec2;

use super::canvas::unit;
use super::def::{Lean, Levers, Material, SettlementsDef, fill};
use crate::biome::Biome;
use crate::content::{ContentError, Select, qualify};
use crate::rng::{Rng, hash3};
use crate::water::cache::{Cache, Weigh};

/// A region's terrain fits, weighed by the storage its family scores own.
#[derive(Clone)]
pub(crate) struct Fits(pub (IVec2, Vec<f32>));

impl Weigh for Fits {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.0.1.capacity() * std::mem::size_of::<f32>()
    }
}

/// How many culture axes there are.
pub const AXES: usize = 8;

/// The eight axes of a culture, each 0 ..= 1 (see `cultures.ron`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Axis {
    /// Temples, shrines on corners, bells, processions.
    Piety,
    /// Walls, towers, barracks, guards, weapon racks.
    Martial,
    /// Markets, stalls, warehouses, foreign goods.
    Trade,
    /// A craft quarter, workshops, tools on walls, signs.
    Craft,
    /// Bunting, lanterns, music, many festivals.
    Festivity,
    /// How open it is to strangers and neighbours' ways (low: closed).
    Outlook,
    /// Rich and poor quarters, a keep or palace, livery.
    Hierarchy,
    /// Libraries, schools, scribes, observatories.
    Lore,
}

impl Axis {
    pub const ALL: [Axis; AXES] = [Axis::Piety, Axis::Martial, Axis::Trade, Axis::Craft, Axis::Festivity, Axis::Outlook, Axis::Hierarchy, Axis::Lore];

    pub fn name(self) -> &'static str {
        match self {
            Axis::Piety => "piety",
            Axis::Martial => "martial",
            Axis::Trade => "trade",
            Axis::Craft => "craft",
            Axis::Festivity => "festivity",
            Axis::Outlook => "outlook",
            Axis::Hierarchy => "hierarchy",
            Axis::Lore => "lore",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }
}

/// Which of settlements.ron's lists an entry weighs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Listing {
    Style,
    House,
    Roof,
    Prop,
    Centrepiece,
    Layout,
    Extra,
}

impl Listing {
    pub const ALL: [Listing; 7] = [Listing::Style, Listing::House, Listing::Roof, Listing::Prop, Listing::Centrepiece, Listing::Layout, Listing::Extra];

    pub fn name(self) -> &'static str {
        match self {
            Listing::Style => "styles",
            Listing::House => "houses",
            Listing::Roof => "roofs",
            Listing::Prop => "props",
            Listing::Centrepiece => "centrepieces",
            Listing::Layout => "layouts",
            Listing::Extra => "extras",
        }
    }
}

/// One weighting of one entry of one list: houses called `hall` ×3, props called `Lamp` ×2.
/// A weight of 0 removes the entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub listing: Listing,
    pub name: String,
    pub mult: f32,
}

/// How a family's names join their Japanese parts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Script {
    /// Katakana, joined by the `kana` table of settlements.ron, a prefix set off by ・.
    #[default]
    Kana,
    /// Kanji, run together as they are ("Sakura" + "gawa" is 桜川), a prefix put straight
    /// on (古桜川) and a hamlet's small word too (松宿).
    Kanji,
}

/// Name parts, in English and Japanese, of the places of a family.
#[derive(Clone, Debug, PartialEq)]
pub struct NamesDef {
    pub script: Script,
    pub first: Vec<(String, String)>,
    pub last: Vec<(String, String)>,
    pub small: Vec<(String, String)>,
    /// Words put before a name ("Old", "Upper"), in English and Japanese, by weight.
    pub prefixes: Vec<(String, String, f32, Lean)>,
    pub small_chance: f32,
    pub prefix_chance: f32,
}

/// The colours of a family, as blocks for the walls, trim and roofs of its places: eight
/// in all, of which a settlement keeps three to five as its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaletteDef {
    pub walls: Material,
    pub trim: Material,
    pub roof: Material,
}

impl PaletteDef {
    pub fn materials(&self) -> [&Material; 3] {
        [&self.walls, &self.trim, &self.roof]
    }

    fn len(&self) -> usize {
        self.materials().iter().map(|m| m.0.len()).sum()
    }
}

/// What a family claims by site rather than by region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Claim {
    /// Only the region field gives it out.
    #[default]
    Region,
    /// Where fresh water is near and trade runs high (river folk).
    Water,
    /// In cities and capitals only (the Old Empire, P57c).
    City,
}

/// A family of cultures.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub name: String,
    /// In English and in Japanese.
    pub title: (String, String),
    /// The biome tags it suits fully, and those it suits at half weight.
    pub biome: Vec<String>,
    pub half: Vec<String>,
    /// The mean annual temperatures it lives in, °C.
    pub temperature: (f32, f32),
    pub weight: f32,
    pub claim: Claim,
    /// Its mean on each axis.
    pub axes: [f32; AXES],
    pub entries: Vec<Entry>,
    pub palette: PaletteDef,
    /// How much it favours each quirk, by name.
    pub quirks: BTreeMap<String, f32>,
    pub names: NamesDef,
}

/// A thing about the ground a quirk needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Need {
    #[default]
    Nothing,
    /// Steep ground (stair towns, rope bridges).
    Relief,
    /// Fresh water near (bridge towns, fishing towns, stilt towns).
    Water,
}

/// A small oddity a settlement may have, on top of its family.
#[derive(Clone, Debug, PartialEq)]
pub struct Quirk {
    pub name: String,
    pub title: (String, String),
    pub weight: f32,
    pub lean: Lean,
    pub need: Need,
    pub axes: [f32; AXES],
    pub entries: Vec<Entry>,
}

/// The region field (`rules.field`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FieldRules {
    /// Blocks across a cell's grid square: about the size of a region.
    pub cell: i32,
    /// How far a cell's middle may stray from its square's, as a share of the square.
    pub jitter: f32,
    /// Sample points a cell looks at on each side of its middle (3 means 3 by 3, 9).
    pub probes: i32,
    /// Blocks from a cell's edge within which the next cell's family mixes in.
    pub border: f32,
    /// The share of the next family at the edge itself and at `border` blocks in.
    pub share: (f32, f32),
}

impl Default for FieldRules {
    fn default() -> Self {
        Self { cell: 6144, jitter: 0.8, probes: 3, border: 512.0, share: (0.45, 0.1) }
    }
}

/// The numbers culture planning reads (`rules` in cultures.ron).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Rules {
    pub field: FieldRules,
    /// How much luck (a factor from 1 − luck to 1 + luck, a smooth field per family
    /// `luck_cell` blocks across) bends which family a cell takes.
    pub luck: f32,
    pub luck_cell: i32,
    /// How much of each neighbouring cell's fit a cell adds to its own.
    pub smooth: f32,
    /// The chance a place with fresh water near and trade at least `claim.1` takes the
    /// river folk (`claim.0`).
    pub claim: (f32, f32),
    /// Blocks across the lattice of the axes' regional drift, and the most it moves one.
    pub drift: (i32, f32),
    /// The most local noise moves an axis, either way.
    pub local: f32,
    /// Mutation: family favourites dropped (least, most; fewer where order is high), the
    /// most borrowed (how many follow `outlook × trade × borrow_scale`), palette colours
    /// kept (least, most), and the chance of a second quirk.
    pub drop: (u32, u32),
    pub borrow: u32,
    pub borrow_scale: f32,
    pub keep: (u32, u32),
    pub second_quirk: f32,
    /// How much of a style's wall, trim and roof weight the palette colours add.
    pub palette: f32,
    /// Axis shifts per unit of a site input (`age`, `wealth`, `order`, `forest`, `water`,
    /// `green`, `martial`, `trade`). Martial and trade are read after the other shifts.
    pub site_shift: BTreeMap<String, AxesFile>,
    /// What a size shifts each axis by, by the size's name (`town`, `city`; P57b and c).
    pub size_shift: BTreeMap<String, AxesFile>,
    /// Martial rises `.1` per level of the land above `.0`; piety by `.1` at level `.0`
    /// and up (P15).
    pub level_martial: (u32, f32),
    pub level_piety: (u32, f32),
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            field: FieldRules::default(),
            luck: 0.6,
            luck_cell: 18432,
            smooth: 0.08,
            claim: (0.35, 0.55),
            drift: (2048, 0.12),
            local: 0.1,
            drop: (1, 3),
            borrow: 2,
            borrow_scale: 3.0,
            keep: (3, 5),
            second_quirk: 0.25,
            palette: 0.5,
            site_shift: BTreeMap::new(),
            size_shift: BTreeMap::new(),
            level_martial: (3, 0.06),
            level_piety: (7, 0.1),
        }
    }
}

/// A value for some of the eight axes; the ones left out are the default of the use.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AxesFile {
    pub piety: Option<f32>,
    pub martial: Option<f32>,
    pub trade: Option<f32>,
    pub craft: Option<f32>,
    pub festivity: Option<f32>,
    pub outlook: Option<f32>,
    pub hierarchy: Option<f32>,
    pub lore: Option<f32>,
}

impl AxesFile {
    fn array(&self, or: f32) -> [f32; AXES] {
        let v = [self.piety, self.martial, self.trade, self.craft, self.festivity, self.outlook, self.hierarchy, self.lore];
        v.map(|x| x.unwrap_or(or))
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct TiltFile {
    styles: BTreeMap<String, f32>,
    houses: BTreeMap<String, f32>,
    roofs: BTreeMap<String, f32>,
    props: BTreeMap<String, f32>,
    centrepieces: BTreeMap<String, f32>,
    layouts: BTreeMap<String, f32>,
    extras: BTreeMap<String, f32>,
}

impl TiltFile {
    fn entries(&self) -> Vec<Entry> {
        let lists = [
            (Listing::Style, &self.styles),
            (Listing::House, &self.houses),
            (Listing::Roof, &self.roofs),
            (Listing::Prop, &self.props),
            (Listing::Centrepiece, &self.centrepieces),
            (Listing::Layout, &self.layouts),
            (Listing::Extra, &self.extras),
        ];
        lists.into_iter().flat_map(|(listing, map)| map.iter().map(move |(name, &mult)| Entry { listing, name: name.clone(), mult })).collect()
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct PaletteFile {
    walls: Vec<Select>,
    trim: Vec<Select>,
    roof: Vec<Select>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrefixFile {
    en: String,
    ja: String,
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamesFile {
    #[serde(default)]
    script: Script,
    first: Vec<(String, String)>,
    last: Vec<(String, String)>,
    #[serde(default)]
    small: Vec<(String, String)>,
    #[serde(default)]
    prefixes: Vec<PrefixFile>,
    /// How often a hamlet takes a small word for its last part.
    #[serde(default = "small_chance")]
    small_chance: f32,
    /// How often a name takes a prefix.
    #[serde(default = "prefix_chance")]
    prefix_chance: f32,
}

fn small_chance() -> f32 {
    0.3
}

fn prefix_chance() -> f32 {
    0.14
}

fn one() -> f32 {
    1.0
}

fn anywhere() -> (f32, f32) {
    (-100.0, 100.0)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FamilyFile {
    name: String,
    title: (String, String),
    #[serde(default)]
    biome: Vec<String>,
    #[serde(default)]
    half: Vec<String>,
    #[serde(default = "anywhere")]
    temperature: (f32, f32),
    #[serde(default = "one")]
    weight: f32,
    #[serde(default)]
    claim: Claim,
    axes: AxesFile,
    #[serde(default)]
    tilt: TiltFile,
    #[serde(default)]
    palette: PaletteFile,
    #[serde(default)]
    quirks: BTreeMap<String, f32>,
    names: NamesFile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuirkFile {
    name: String,
    title: (String, String),
    #[serde(default = "one")]
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    #[serde(default)]
    need: Need,
    #[serde(default)]
    axes: AxesFile,
    #[serde(default)]
    tilt: TiltFile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CulturesFile {
    rules: Option<RulesFile>,
    #[serde(default)]
    families: Vec<FamilyFile>,
    #[serde(default)]
    quirks: Vec<QuirkFile>,
}

/// Later packages replace only the rule fields they name.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct FieldFile {
    cell: Option<i32>,
    jitter: Option<f32>,
    probes: Option<i32>,
    border: Option<f32>,
    share: Option<(f32, f32)>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct RulesFile {
    field: Option<FieldFile>,
    luck: Option<f32>,
    luck_cell: Option<i32>,
    smooth: Option<f32>,
    claim: Option<(f32, f32)>,
    drift: Option<(i32, f32)>,
    local: Option<f32>,
    drop: Option<(u32, u32)>,
    borrow: Option<u32>,
    borrow_scale: Option<f32>,
    keep: Option<(u32, u32)>,
    second_quirk: Option<f32>,
    palette: Option<f32>,
    site_shift: Option<BTreeMap<String, AxesFile>>,
    size_shift: Option<BTreeMap<String, AxesFile>>,
    level_martial: Option<(u32, f32)>,
    level_piety: Option<(u32, f32)>,
}

/// Every package's `cultures.ron` together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CulturesDef {
    pub rules: Rules,
    pub families: Vec<Family>,
    pub quirks: Vec<Quirk>,
    /// Which package set each single-valued rule, for draft 91 load notes.
    setters: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

/// A settlement's second family, mixed in near a border (or by a site claim).
#[derive(Clone, Debug, PartialEq)]
pub struct Second {
    pub family: usize,
    /// How much of every weight is the second family's, 0 ..= 1.
    pub share: f32,
    pub entries: Vec<Entry>,
}

/// A settlement's culture: its family, its axes, its mutated entries and its quirks.
#[derive(Clone, Debug, PartialEq)]
pub struct Culture {
    pub family: usize,
    pub second: Option<Second>,
    /// Whether a site claim made this its family (the region's became the second).
    pub claimed: bool,
    pub axes: [f32; AXES],
    /// The family's entries after dropping and borrowing.
    pub entries: Vec<Entry>,
    pub dropped: Vec<Entry>,
    /// What was borrowed, and from which family.
    pub borrowed: Vec<(usize, Entry)>,
    /// The palette colours it kept.
    pub palette: PaletteDef,
    pub quirks: Vec<usize>,
    /// The quirks' own entries, which multiply on top.
    pub quirk_entries: Vec<Entry>,
}

impl Culture {
    /// What this culture multiplies the weight of the `name`d entry of `listing` by: the
    /// family's, blended with the second family's by its share, times the quirks'.
    pub fn mult(&self, listing: Listing, name: &str) -> f32 {
        let of = |entries: &[Entry]| -> f32 { entries.iter().filter(|e| e.listing == listing && e.name == name).map(|e| e.mult).product() };
        let main = of(&self.entries);
        let blended = match &self.second {
            Some(s) => main * (1.0 - s.share) + of(&s.entries) * s.share,
            None => main,
        };
        blended * of(&self.quirk_entries)
    }
}

impl Weigh for Culture {
    fn bytes(&self) -> usize {
        let entries = |all: &Vec<Entry>| all.capacity() * std::mem::size_of::<Entry>() + all.iter().map(|e| e.name.capacity()).sum::<usize>();
        std::mem::size_of::<Self>() + entries(&self.entries) + entries(&self.dropped)
            + entries(&self.quirk_entries) + self.second.as_ref().map_or(0, |s| entries(&s.entries))
            + self.borrowed.capacity() * std::mem::size_of::<(usize, Entry)>() + self.borrowed.iter().map(|(_, e)| e.name.capacity()).sum::<usize>()
            + self.quirks.capacity() * std::mem::size_of::<usize>()
            + self.palette.materials().iter().map(|m| m.0.capacity() * std::mem::size_of::<(String, f32)>() + m.0.iter().map(|p| p.0.capacity()).sum::<usize>()).sum::<usize>()
    }
}

/// The stream of a cell's jitter, of its luck, of an axis's drift, and of the roll.
const CELL_JITTER: u64 = 0xce_11_0001;
const CELL_LUCK: u64 = 0xce_11_1000;
const DRIFT_FIELD: u64 = 0xd7_1f_0000;
const ROLL: u64 = 0xc017_5eed;

/// What a settlement's culture rolls on.
pub struct Site<'a> {
    pub seed: u64,
    /// Its grid square, and where it stands.
    pub cell: IVec2,
    pub at: IVec2,
    /// The level of the land (P15).
    pub level: u32,
    /// Whether fresh water is near.
    pub fresh: bool,
    /// How steep the ground is, 0 ..= 1.
    pub relief: f32,
    /// How wooded the ground is, 0 ..= 1.
    pub forest: f32,
    /// Its levers (wealth, age, order, crowding, green; the axes are drawn here).
    pub levers: Levers,
    /// The family a region's cell names ([`CulturesDef::cell_family`]).
    pub cell_family: &'a dyn Fn(IVec2) -> usize,
}

/// Smooth value noise in 0 ..= 1 over a lattice of `cell` blocks, for stream `salt`.
fn noise(seed: u64, salt: u64, at: IVec2, cell: i32) -> f32 {
    let (fx, fz) = (at.x as f32 / cell as f32, at.y as f32 / cell as f32);
    let (i, j) = (fx.floor(), fz.floor());
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (tx, tz) = (smooth(fx - i), smooth(fz - j));
    let at = |di: i32, dj: i32| unit(hash3(seed ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15), i as i32 + di, 0, j as i32 + dj));
    let (a, b) = (at(0, 0) + (at(1, 0) - at(0, 0)) * tx, at(0, 1) + (at(1, 1) - at(0, 1)) * tx);
    a + (b - a) * tz
}

impl CulturesDef {
    /// Adds a package's `cultures.ron`: named rules replace earlier values with a note;
    /// families and quirks accumulate.
    pub fn extend(&mut self, at: &str, package: &str, source: &str) -> Result<(), ContentError> {
        let file: CulturesFile = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(source)
            .map_err(|e| ContentError(format!("{at}: {e}")))?;
        if let Some(set) = file.rules {
            let mut changed = Vec::new();
            macro_rules! lay {
                ($set:ident, $target:expr, $prefix:literal, $($key:ident),+ $(,)?) => {
                    $(if let Some(value) = $set.$key {
                        $target.$key = value;
                        changed.push(concat!($prefix, stringify!($key)).to_owned());
                    })+
                };
            }
            if let Some(field) = set.field {
                lay!(field, self.rules.field, "rules.field.", cell, jitter, probes, border, share);
            }
            lay!(set, self.rules, "rules.", luck, luck_cell, smooth, claim, drift, local, drop,
                borrow, borrow_scale, keep, second_quirk, palette, level_martial, level_piety);
            for (label, shifts, targets) in [
                ("size_shift", set.size_shift, &mut self.rules.size_shift),
                ("site_shift", set.site_shift, &mut self.rules.site_shift),
            ] {
                for (size, shift) in shifts.into_iter().flatten() {
                    let target = targets.entry(size.clone()).or_default();
                    macro_rules! axis {
                        ($($a:ident),+) => {
                            $(if let Some(value) = shift.$a {
                                target.$a = Some(value);
                                changed.push(format!("rules.{label}.{size}.{}", stringify!($a)));
                            })+
                        };
                    }
                    axis!(piety, martial, trade, craft, festivity, outlook, hierarchy, lore);
                }
            }
            for path in changed {
                if let Some(earlier) = self.setters.insert(path.clone(), package.to_owned()) {
                    self.notes.push(format!("{at} replaces the cultures.ron {earlier} set ({path})"));
                }
            }
        }
        for f in file.families {
            let m = |list: &[Select]| -> Result<Material, ContentError> {
                list.iter()
                    .map(|s| match s {
                        Select::Name(name, weight) => Ok((qualify(package, name), *weight)),
                        Select::Tag(tag, _) => Err(ContentError(format!("{at}: palettes name their blocks; Tag(\"{tag}\") is not allowed"))),
                    })
                    .collect::<Result<_, _>>()
                    .map(Material)
            };
            let prefixes = f
                .names
                .prefixes
                .into_iter()
                .map(|p| Ok((p.en, p.ja, p.weight, Lean::parse(at, &p.lean)?)))
                .collect::<Result<_, ContentError>>()?;
            self.families.push(Family {
                name: f.name,
                title: f.title,
                biome: f.biome,
                half: f.half,
                temperature: f.temperature,
                weight: f.weight,
                claim: f.claim,
                axes: f.axes.array(0.5),
                entries: f.tilt.entries(),
                palette: PaletteDef { walls: m(&f.palette.walls)?, trim: m(&f.palette.trim)?, roof: m(&f.palette.roof)? },
                quirks: f.quirks,
                names: NamesDef {
                    script: f.names.script,
                    first: f.names.first,
                    last: f.names.last,
                    small: f.names.small,
                    prefixes,
                    small_chance: f.names.small_chance,
                    prefix_chance: f.names.prefix_chance,
                },
            });
        }
        for q in file.quirks {
            self.quirks.push(Quirk {
                lean: Lean::parse(at, &q.lean)?,
                axes: q.axes.array(0.0),
                entries: q.tilt.entries(),
                name: q.name,
                title: q.title,
                weight: q.weight,
                need: q.need,
            });
        }
        Ok(())
    }

    /// Checks the numbers and that every entry names something in settlements.ron and every
    /// palette block exists.
    pub fn check(&self, def: &SettlementsDef, registry: &BlockRegistry) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("cultures.ron: {what}")));
        let r = &self.rules;
        let f = &r.field;
        if !(1024..=1_048_576).contains(&f.cell) || !(0.0..=1.0).contains(&f.jitter) || !(1..=7).contains(&f.probes) || !f.border.is_finite() || f.border <= 0.0 || f.share.0 < f.share.1 || !(0.0..=1.0).contains(&f.share.1) || !(0.0..=1.0).contains(&f.share.0) {
            return fail(format!("rules.field: a cell of 1024 blocks or more, a jitter in [0, 1], 1 to 7 probes, a border above nothing and shares in [0, 1] falling from the edge in: {f:?}"));
        }
        if r.drift.0 < 256 || !(0.0..=1.0).contains(&r.drift.1) || !(0.0..=1.0).contains(&r.local) || !(0.0..=1.0).contains(&r.luck) || r.luck_cell < 1024 || !(0.0..=1.0).contains(&r.smooth) || r.drop.0 > r.drop.1 || r.drop.1 > 32 || r.borrow > 32 || !(0.0..=32.0).contains(&r.borrow_scale) || r.keep.0 > r.keep.1 || r.keep.0 == 0 || r.keep.1 > 64 {
            return fail(format!("rules: a drift cell of 256 or more, drift and local at least nothing, luck in [0, 1], drop and keep as (least, most) with at least one colour kept: {r:?}"));
        }
        if !(0.0..=1.0).contains(&r.second_quirk) || !(0.0..=4.0).contains(&r.palette) || !(0.0..=1.0).contains(&r.claim.0) || !(0.0..=1.0).contains(&r.claim.1) {
            return fail(format!("rules: chances in [0, 1] and a palette share to 4: {r:?}"));
        }
        if self.families.len() < 2 {
            return fail("needs two families or more".into());
        }
        if !(0.0..=1.0).contains(&r.level_martial.1) || !(0.0..=1.0).contains(&r.level_piety.1)
            || r.size_shift.values().chain(r.site_shift.values()).any(|s| s.array(0.0).iter().any(|v| !(-1.0..=1.0).contains(v)))
            || r.site_shift.keys().any(|s| !["age", "wealth", "order", "forest", "water", "green", "martial", "trade"].contains(&s.as_str()))
        {
            return fail("level and size shifts must be finite and in range".into());
        }
        let known = |listing: Listing| -> BTreeSet<String> {
            match listing {
                Listing::Style => def.styles.iter().map(|s| s.name.clone()).collect(),
                Listing::House => def.houses.iter().map(|h| h.name.clone()).collect(),
                Listing::Roof => def.roofs.iter().map(|r| format!("{:?}", r.shape)).collect(),
                Listing::Prop => def.props.iter().map(|p| p.kind.label()).collect(),
                Listing::Centrepiece => def.centrepieces.iter().map(|c| c.kind.label()).collect(),
                Listing::Layout => def.layouts.iter().map(|l| l.name.clone()).collect(),
                Listing::Extra => def.extras.iter().map(|e| format!("{:?}", e.name)).collect(),
            }
        };
        let lists: Vec<(Listing, BTreeSet<String>)> = Listing::ALL.into_iter().map(|l| (l, known(l))).collect();
        let entries_ok = |who: &str, entries: &[Entry]| -> Result<(), ContentError> {
            let mut seen = FxHashSet::default();
            for e in entries {
                let names = &lists.iter().find(|(l, _)| *l == e.listing).unwrap().1;
                if !names.contains(&e.name) {
                    return Err(ContentError(format!("cultures.ron: {who} weighs {} `{}`, which settlements.ron does not have", e.listing.name(), e.name)));
                }
                if !(0.0..=16.0).contains(&e.mult) {
                    return Err(ContentError(format!("cultures.ron: {who} weighs {} `{}` by {}, past 0 to 16", e.listing.name(), e.name, e.mult)));
                }
                if !seen.insert((e.listing, e.name.clone())) {
                    return Err(ContentError(format!("cultures.ron: {who} weighs {} `{}` twice", e.listing.name(), e.name)));
                }
            }
            Ok(())
        };
        let woods: BTreeSet<&str> = def.styles.iter().flat_map(|s| s.woods.iter().map(|(w, _)| w.as_str())).collect();
        let mut names = BTreeSet::new();
        for f in &self.families {
            if !names.insert(f.name.as_str()) {
                return fail(format!("two families are called `{}`", f.name));
            }
            if !f.weight.is_finite() || f.weight < 0.0 || f.axes.iter().any(|a| !(0.0..=1.0).contains(a)) || !f.temperature.0.is_finite() || !f.temperature.1.is_finite() || f.temperature.0 > f.temperature.1 {
                return fail(format!("family `{}` needs a weight of nothing or more, axes in [0, 1] and temperatures from low to high", f.name));
            }
            if f.title.0.is_empty() || f.title.1.is_empty() {
                return fail(format!("family `{}` needs a title in English and Japanese", f.name));
            }
            let n = &f.names;
            if n.first.len() < 2 || n.last.len() < 2 || n.first.iter().chain(&n.last).chain(&n.small).any(|p| p.0.is_empty() || p.1.is_empty()) {
                return fail(format!("family `{}` needs two or more first and last name parts, each in English and Japanese", f.name));
            }
            if !(0.0..=1.0).contains(&n.small_chance) || !(0.0..=1.0).contains(&n.prefix_chance) {
                return fail(format!("family `{}`: name chances are in [0, 1]", f.name));
            }
            if n.prefixes.iter().any(|p| p.0.is_empty() || p.1.is_empty() || !p.2.is_finite() || p.2 < 0.0)
                || f.quirks.values().any(|v| !v.is_finite() || *v < 0.0)
            {
                return fail(format!("family `{}`: prefixes and quirk weights need finite non-negative weights and both languages", f.name));
            }
            entries_ok(&format!("family `{}`", f.name), &f.entries)?;
            if f.palette.len() < r.keep.1 as usize {
                return fail(format!("family `{}` has {} palette colours, fewer than the {} a place may keep", f.name, f.palette.len(), r.keep.1));
            }
            for material in f.palette.materials() {
                for (name, w) in &material.0 {
                    if !w.is_finite() || *w <= 0.0 {
                        return fail(format!("family `{}`: a palette weight of nothing or less", f.name));
                    }
                    for wood in &woods {
                        let one = fill(name, wood, wood);
                        if registry.id(&one).is_none() {
                            return fail(format!("family `{}`'s palette names `{one}`, which is no block", f.name));
                        }
                    }
                }
            }
            if let Some(q) = f.quirks.keys().find(|q| !self.quirks.iter().any(|k| &k.name == *q)) {
                return fail(format!("family `{}` favours the quirk `{q}`, which there is none of", f.name));
            }
        }
        if !self.families.iter().any(|f| f.claim == Claim::Region && f.weight > 0.0) {
            return fail("needs a family the region field can give".into());
        }
        let mut quirks = BTreeSet::new();
        for q in &self.quirks {
            if !quirks.insert(q.name.as_str()) {
                return fail(format!("two quirks are called `{}`", q.name));
            }
            if !q.weight.is_finite() || q.weight <= 0.0 || q.title.0.is_empty() || q.title.1.is_empty() || q.axes.iter().any(|v| !(-1.0..=1.0).contains(v)) {
                return fail(format!("quirk `{}` needs a weight above nothing and a title in English and Japanese", q.name));
            }
            entries_ok(&format!("quirk `{}`", q.name), &q.entries)?;
        }
        Ok(())
    }

    /// How well family `f` suits a place: nothing outside its temperatures, 1 where its
    /// biome tags match (a little more for each further), a half where its `half` tags do.
    fn fit(&self, f: &Family, biome: &Biome, temperature: f32) -> f32 {
        if f.claim != Claim::Region || temperature < f.temperature.0 || temperature > f.temperature.1 {
            return 0.0;
        }
        match f.biome.iter().filter(|t| biome.has_tag(t)).count() {
            0 if f.half.iter().any(|t| biome.has_tag(t)) => 0.5,
            0 => 0.0,
            n => 1.0 + 0.2 * (n - 1) as f32,
        }
    }

    /// The middle of the cell whose grid square is `g`: jittered inside the square.
    fn cell_point(&self, seed: u64, g: IVec2) -> IVec2 {
        let f = &self.rules.field;
        let h = hash3(seed ^ CELL_JITTER, g.x, 0, g.y);
        let (jx, jy) = (unit(h) - 0.5, unit(h.rotate_left(24)) - 0.5);
        let c = f.cell as f32;
        IVec2::new(((g.x as f32 + 0.5 + jx * f.jitter) * c) as i32, ((g.y as f32 + 0.5 + jy * f.jitter) * c) as i32)
    }

    /// How well each family suits the cell with grid square `g`: the fits of its sample
    /// points summed, and the cell's middle.
    pub fn cell_fits<'b>(&self, seed: u64, g: IVec2, probe: &dyn Fn(IVec2) -> (&'b Biome, f32)) -> (IVec2, Vec<f32>) {
        let f = &self.rules.field;
        let middle = self.cell_point(seed, g);
        let reach = f.cell as f32 * 0.35;
        let n = f.probes;
        let mut score = vec![0.0f32; self.families.len()];
        for j in 0..n {
            for i in 0..n {
                let t = |k: i32| if n == 1 { 0.0 } else { (k as f32 / (n - 1) as f32 * 2.0 - 1.0) * reach };
                let p = middle + IVec2::new(t(i) as i32, t(j) as i32);
                let (biome, temperature) = probe(p);
                for (k, fam) in self.families.iter().enumerate() {
                    score[k] += self.fit(fam, biome, temperature);
                }
            }
        }
        (middle, score)
    }

    /// The family a cell with these `fits` names: the best by fit, weight and luck. Luck is
    /// a smooth field per family, `luck_cell` blocks across, so neighbouring cells are
    /// lucky together and a family holds a whole stretch of country, not one cell. Where
    /// none fits, the first family the region field can give out.
    pub fn cell_pick(&self, seed: u64, middle: IVec2, fits: &[f32]) -> usize {
        let score: Vec<f32> = fits
            .iter()
            .zip(&self.families)
            .enumerate()
            .map(|(k, (fit, fam))| fit * fam.weight * (1.0 + (noise(seed, CELL_LUCK + k as u64, middle, self.rules.luck_cell) * 2.0 - 1.0) * self.rules.luck))
            .collect();
        let best = (0..score.len()).max_by(|&a, &b| score[a].total_cmp(&score[b]).then(b.cmp(&a))).unwrap_or(0);
        if score[best] > 0.0 { best } else { self.families.iter().position(|f| f.claim == Claim::Region && f.weight > 0.0).unwrap_or(0) }
    }

    /// The family the cell with grid square `g` names: its own fits plus `smooth` times
    /// those of its eight neighbours (so a family holds country across cells), by
    /// [`Self::cell_pick`]. `fits_of` gives any cell's [`Self::cell_fits`], remembered
    /// by the caller (asking the land costs).
    pub fn cell_family(&self, seed: u64, g: IVec2, fits_of: &dyn Fn(IVec2) -> (IVec2, Vec<f32>)) -> usize {
        let (middle, mut sum) = fits_of(g);
        for dz in -1..=1 {
            for dx in -1..=1 {
                if (dx, dz) != (0, 0) {
                    for (s, v) in sum.iter_mut().zip(fits_of(g + IVec2::new(dx, dz)).1) {
                        *s += v * self.rules.smooth;
                    }
                }
            }
        }
        self.cell_pick(seed, middle, &sum)
    }

    /// Reuses terrain probes under a byte budget, without depending on cache order.
    pub(crate) fn cached_family<'b>(&self, seed: u64, g: IVec2, cache: &Cache<IVec2, Fits>, probe: &dyn Fn(IVec2) -> (&'b Biome, f32)) -> usize {
        self.cell_family(seed, g, &|g| cache.get(g, || Fits(self.cell_fits(seed, g, probe))).0)
    }

    /// The two nearest cells to `at` (their grid squares), and how far `at` is from the
    /// edge between them, in blocks.
    fn cells_near(&self, seed: u64, at: IVec2) -> ((IVec2, IVec2), f32) {
        let c = self.rules.field.cell;
        let g = IVec2::new(at.x.div_euclid(c), at.y.div_euclid(c));
        let mut found: Vec<(f32, IVec2)> = Vec::with_capacity(25);
        for dz in -2..=2 {
            for dx in -2..=2 {
                let cell = g + IVec2::new(dx, dz);
                found.push((self.cell_point(seed, cell).as_vec2().distance(at.as_vec2()), cell));
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1.x, a.1.y).cmp(&(b.1.x, b.1.y))));
        let ((d0, a), (d1, b)) = (found[0], found[1]);
        let apart = self.cell_point(seed, a).as_vec2().distance(self.cell_point(seed, b).as_vec2()).max(1.0);
        ((a, b), (d1 * d1 - d0 * d0) / (2.0 * apart))
    }

    /// The family of the region at `at`, the next region's and how far `at` is from the
    /// edge between them in blocks.
    pub fn region_at(&self, seed: u64, at: IVec2, cell_family: &dyn Fn(IVec2) -> usize) -> (usize, usize, f32) {
        let ((a, b), edge) = self.cells_near(seed, at);
        (cell_family(a), cell_family(b), edge)
    }

    /// The drift of `axis` at a place, -`amount` ..= `amount`: a field of its own.
    fn drift(&self, seed: u64, axis: Axis, at: IVec2) -> f32 {
        (noise(seed, DRIFT_FIELD + axis as u64, at, self.rules.drift.0) * 2.0 - 1.0) * self.rules.drift.1
    }

    /// Rolls the culture of the settlement at `site`: its family (and second), axes before
    /// size ([`Self::finish`]), the family's entries dropped and borrowed and its palette
    /// kept.
    pub fn roll(&self, site: &Site) -> Culture {
        let mut rng = Rng::new(site.seed, ROLL, site.cell.x, site.cell.y);
        let r = &self.rules;
        let (region, next, edge) = self.region_at(site.seed, site.at, site.cell_family);
        // Near the edge of its region, a share of the next family mixes in, more the nearer.
        let mut family = region;
        let mut second = None;
        let f = &r.field;
        if next != region && edge < f.border {
            let t = (edge.max(0.0) / f.border).clamp(0.0, 1.0);
            second = Some((next, f.share.0 + (f.share.1 - f.share.0) * t));
        }
        // The levers and axes this place will have, before any claim, for the claim's sake.
        let mut axes = [0.0; AXES];
        for axis in Axis::ALL {
            let local = (rng.unit() * 2.0 - 1.0) * r.local;
            axes[axis as usize] = self.families[family].axes[axis as usize] + self.drift(site.seed, axis, site.at) + local;
        }
        // River folk claim a place by its site: fresh water near and trade high.
        let mut claimed = false;
        let water_trade = r.site_shift.get("water").map_or(0.0, |s| s.array(0.0)[Axis::Trade as usize]);
        if site.fresh && axes[Axis::Trade as usize] + water_trade >= r.claim.1 && rng.chance(r.claim.0)
            && let Some(river) = self.families.iter().position(|f| f.claim == Claim::Water)
            && river != region
        {
            second = Some((region, 0.3));
            family = river;
            claimed = true;
        }
        let f = &self.families[family];
        // Axes: the family's mean, regional drift and local noise (kept from above, rebased
        // on a claiming family's mean), and shifts from the site.
        if claimed {
            for i in 0..AXES {
                axes[i] += f.axes[i] - self.families[region].axes[i];
            }
        }
        let l = site.levers;
        let a = |x: Axis| x as usize;
        axes[a(Axis::Piety)] += if site.level >= r.level_piety.0 { r.level_piety.1 } else { 0.0 };
        axes[a(Axis::Martial)] += r.level_martial.1 * site.level.saturating_sub(r.level_martial.0) as f32;
        let shifted = |axes: &mut [f32; AXES], source: &str, amount: f32| {
            if let Some(shift) = r.site_shift.get(source) {
                for (value, slope) in axes.iter_mut().zip(shift.array(0.0)) {
                    *value += amount * slope;
                }
            }
        };
        for (source, amount) in [
            ("age", l.get(super::def::Lever::Age)),
            ("wealth", l.get(super::def::Lever::Wealth)),
            ("order", l.get(super::def::Lever::Order)),
            ("forest", site.forest),
            ("water", if site.fresh { 1.0 } else { 0.0 }),
        ] {
            shifted(&mut axes, source, amount);
        }
        axes[a(Axis::Martial)] = axes[a(Axis::Martial)].clamp(0.0, 1.0);
        axes[a(Axis::Trade)] = axes[a(Axis::Trade)].clamp(0.0, 1.0);
        let (martial, trade) = (axes[a(Axis::Martial)], axes[a(Axis::Trade)]);
        shifted(&mut axes, "green", l.get(super::def::Lever::Green));
        shifted(&mut axes, "martial", martial);
        shifted(&mut axes, "trade", trade);
        for v in &mut axes {
            *v = v.clamp(0.0, 1.0);
        }

        let order = l.get(super::def::Lever::Order);
        let mut entries = f.entries.clone();
        // Drop one to three favourites (fewer where order is high): what this place never
        // took up.
        let mut drop = rng.range(r.drop.0 as i32, r.drop.1 as i32 + 1) as usize;
        if rng.chance(order) {
            drop = drop.saturating_sub(1).max(r.drop.0 as usize);
        }
        let mut dropped = Vec::new();
        for _ in 0..drop {
            let favourites: Vec<usize> = (0..entries.len()).filter(|&i| entries[i].mult > 1.0).collect();
            if favourites.len() <= 1 {
                break;
            }
            let i = favourites[rng.below(favourites.len() as u32) as usize];
            dropped.push(entries.remove(i));
        }
        // Borrow nought to two favourites from the second family or a random one, as many as
        // outlook × trade × `borrow_scale` suggests.
        let want = (axes[a(Axis::Outlook)] * axes[a(Axis::Trade)] * r.borrow_scale).clamp(0.0, r.borrow as f32);
        let chance = if r.borrow == 0 { 0.0 } else { want / r.borrow as f32 };
        let mut borrowed = Vec::new();
        for _ in 0..r.borrow {
            if !rng.chance(chance) {
                continue;
            }
            let donor = match &second {
                Some((s, _)) if rng.chance(0.7) => *s,
                _ => rng.below(self.families.len() as u32) as usize,
            };
            if donor == family {
                continue;
            }
            let theirs: Vec<&Entry> =
                self.families[donor].entries.iter().filter(|e| e.mult > 1.0 && !entries.iter().any(|m| m.listing == e.listing && m.name == e.name)).collect();
            if theirs.is_empty() {
                continue;
            }
            let e = theirs[rng.below(theirs.len() as u32) as usize].clone();
            entries.push(e.clone());
            borrowed.push((donor, e));
        }
        // Keep three to five of the family's colours as its own, by weight.
        let mut pool: Vec<(usize, usize, f32)> = Vec::new();
        for (kind, material) in f.palette.materials().into_iter().enumerate() {
            pool.extend(material.0.iter().enumerate().map(|(i, (_, w))| (kind, i, *w)));
        }
        let keep = (rng.range(r.keep.0 as i32, r.keep.1 as i32 + 1) as usize).min(pool.len());
        let mut kept: Vec<Vec<bool>> = f.palette.materials().iter().map(|m| vec![false; m.0.len()]).collect();
        for _ in 0..keep {
            let total: f32 = pool.iter().map(|p| p.2).sum();
            let mut t = rng.unit() * total;
            let mut at = pool.len() - 1;
            for (i, p) in pool.iter().enumerate() {
                if t < p.2 {
                    at = i;
                    break;
                }
                t -= p.2;
            }
            let (kind, i, _) = pool.remove(at);
            kept[kind][i] = true;
        }
        let pick = |kind: usize, material: &Material| Material(material.0.iter().enumerate().filter(|(i, _)| kept[kind][*i]).map(|(_, e)| e.clone()).collect());
        let palette = PaletteDef { walls: pick(0, &f.palette.walls), trim: pick(1, &f.palette.trim), roof: pick(2, &f.palette.roof) };
        let second = second.map(|(s, share)| Second { family: s, share, entries: self.families[s].entries.clone() });
        Culture { family, second, claimed, axes, entries, dropped, borrowed, palette, quirks: Vec::new(), quirk_entries: Vec::new() }
    }

    /// Settles what the size and the levers decide: the size's shift of each axis (`town`,
    /// `city`), then a quirk, now and then a second, by weight leaning on the final axes and
    /// levers, on how the family favours it and on the ground it needs.
    pub fn finish(&self, culture: &mut Culture, site: &Site, size: &str) {
        if let Some(shift) = self.rules.size_shift.get(size) {
            for (a, s) in culture.axes.iter_mut().zip(shift.array(0.0)) {
                *a = (*a + s).clamp(0.0, 1.0);
            }
        }
        let mut rng = Rng::new(site.seed, ROLL + 1, site.cell.x, site.cell.y);
        let character = Levers { levers: site.levers.levers, axes: culture.axes };
        let family = &self.families[culture.family];
        let want = 1 + usize::from(rng.chance(self.rules.second_quirk));
        let mut weights: Vec<(usize, f32)> = self
            .quirks
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let need = match q.need {
                    Need::Nothing => 1.0,
                    Need::Relief => site.relief,
                    Need::Water => if site.fresh { 1.0 } else { 0.0 },
                };
                (i, q.lean.weigh(q.weight, &character) * family.quirks.get(&q.name).copied().unwrap_or(1.0) * need)
            })
            .collect();
        for _ in 0..want {
            let total: f32 = weights.iter().map(|(_, w)| w.max(0.0)).sum();
            if total <= 0.0 {
                break;
            }
            let mut t = rng.unit() * total;
            let mut at = weights.len() - 1;
            for (k, (_, w)) in weights.iter().enumerate() {
                let w = w.max(0.0);
                if t < w {
                    at = k;
                    break;
                }
                t -= w;
            }
            let (chosen, _) = weights.remove(at);
            let q = &self.quirks[chosen];
            for (a, s) in culture.axes.iter_mut().zip(q.axes) {
                *a = (*a + s).clamp(0.0, 1.0);
            }
            culture.quirk_entries.extend(q.entries.iter().cloned());
            culture.quirks.push(chosen);
        }
    }
}
