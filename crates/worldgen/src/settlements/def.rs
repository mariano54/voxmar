//! What settlements are made of (`settlements.ron`): the lists generation rolls from and
//! the levers each entry leans on.

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;
use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::content::{ContentError, Select, qualify};

/// The five levers every settlement draws, each 0 ..= 1 (see `settlements.ron`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lever {
    Wealth,
    Age,
    Order,
    Crowding,
    Green,
}

impl Lever {
    pub const ALL: [Lever; 5] = [Lever::Wealth, Lever::Age, Lever::Order, Lever::Crowding, Lever::Green];

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Lever::Wealth => "wealth",
            Lever::Age => "age",
            Lever::Order => "order",
            Lever::Crowding => "crowding",
            Lever::Green => "green",
        }
    }
}

/// A settlement's levers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Levers(pub [f32; 5]);

impl Levers {
    pub fn get(&self, lever: Lever) -> f32 {
        self.0[lever as usize]
    }
}

/// How much an entry's weight follows the levers: each lean multiplies it by
/// `1 + lean × (2 × lever − 1)`, never below nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lean(pub Vec<(Lever, f32)>);

impl Lean {
    fn parse(at: &str, map: &BTreeMap<String, f32>) -> Result<Self, ContentError> {
        map.iter()
            .map(|(name, &lean)| {
                Lever::parse(name)
                    .map(|lever| (lever, lean))
                    .ok_or_else(|| ContentError(format!("{at}: no lever is called `{name}` (wealth, age, order, crowding, green)")))
            })
            .collect::<Result<_, _>>()
            .map(Lean)
    }

    pub fn weigh(&self, weight: f32, levers: &Levers) -> f32 {
        self.0.iter().fold(weight, |w, &(lever, lean)| w * (1.0 + lean * (2.0 * levers.get(lever) - 1.0)).max(0.0))
    }
}

/// Places a settlement always or sometimes has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Special {
    Well,
    Trader,
    Inn,
    Smithy,
    Stall,
    Shrine,
    Field,
    Pen,
    Orchard,
    Woodyard,
}

/// How lanes are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Shape {
    Spokes,
    Line,
    Green,
    Scatter,
    Fork,
    /// Four lanes meeting square on at the well.
    Crossroads,
    /// A lane round the well, houses facing in, a few lanes leading out.
    Ring,
    /// One street along the bank of the water nearby.
    Riverside,
    /// Streets along the slope, one above another.
    Terraces,
}

/// Roofs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Roof {
    Gable,
    Hip,
    Flat,
    Shed,
    /// A gable with its ends hipped near the top.
    HalfHip,
    /// A gable whose ridge sits near the front: short in front, long behind.
    Saltbox,
    /// A barn's roof: steep low down, shallow up top.
    Gambrel,
    /// A cone over a round house (a pyramid over a square one).
    Conical,
}

/// A house's outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Form {
    /// Four walls.
    #[default]
    Box,
    /// A circle of wall under a cone.
    Round,
    /// All roof, down to the ground.
    AFrame,
    /// Rooms round an open court.
    Courtyard,
    /// A shop opening on the street, its counter under an awning, rooms above.
    ShopFront,
}

/// What stands inside a working building.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Fitting {
    /// A stone oven round a fire.
    Oven,
    /// A millstone in the middle.
    Millstone,
    /// Vats of water let into the floor.
    Vats,
    /// Drying racks along the walls.
    Racks,
    /// Stalls for animals down both sides.
    Stalls,
    /// Bins and stacked stores.
    Bins,
    /// A smoking fire in the middle.
    Smoke,
    /// An altar at the back between two lights.
    Altar,
    /// Benches in rows, an aisle down the middle.
    Pews,
    /// A boat on the floor.
    Boat,
    /// A counter across the room.
    Counter,
}

/// The one great thing a village is known by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Centre {
    GreatTree,
    StoneCircle,
    WatchtowerRuin,
    Windmill,
    BellFrame,
    FallenStatue,
    Maypole,
    MarketCross,
    Fountain,
    Beacon,
    StandingStone,
    DuckPond,
}

impl Centre {
    /// The blocks it stands on, across and deep, before it is turned.
    pub fn extent(self) -> (i32, i32) {
        match self {
            Centre::GreatTree => (13, 13),
            Centre::StoneCircle => (11, 11),
            Centre::WatchtowerRuin | Centre::Fountain => (7, 7),
            Centre::Windmill => (11, 7),
            Centre::BellFrame => (5, 3),
            Centre::FallenStatue => (11, 5),
            Centre::Maypole | Centre::MarketCross | Centre::Beacon | Centre::StandingStone => (5, 5),
            Centre::DuckPond => (11, 9),
        }
    }
}

/// Where a centrepiece stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Spot {
    /// On the square or green.
    #[default]
    Square,
    /// Out at the end of a lane.
    Edge,
}

/// What a house may have besides walls and a roof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Extra {
    Chimney,
    Porch,
    Shed,
    FlowerBoxes,
    Garden,
    Hedge,
    Lamp,
    Woodpile,
    Moss,
    Wing,
    /// A window standing out of the roof's front slope.
    Dormer,
    /// A balcony over the door, on the floor above.
    Balcony,
    /// A stair up the outside to the floor above.
    OutsideStair,
    /// Shutters beside the windows.
    Shutters,
    /// A cloth awning over the front.
    Awning,
    /// A board hung by the door.
    SignBoard,
    /// A barrel at a corner.
    RainBarrel,
    /// A vane on the roof's peak.
    Weathervane,
    /// A dovecote on a pole behind.
    Dovecote,
    /// Herbs hung to dry under the eaves.
    HangingHerbs,
}

/// What stands in yards and by lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Prop {
    Woodpile,
    Crates,
    StonePile,
    FlowerBed,
    HerbPlanter,
    Campfire,
    Lamp,
    Bush,
    Compost,
    Boulder,
    /// A hand cart on two wheels.
    Cart,
    Barrel,
    HayBale,
    /// Washing hung on a line between two posts.
    LaundryLine,
    /// A few lengths of rail.
    FenceRun,
    Scarecrow,
    /// A straw beehive on a stump.
    Skep,
    Anvil,
    Grindstone,
    Washtub,
    Sacks,
    ChickenCoop,
    Signpost,
    /// A boat drawn up on the bank (by water only).
    Boat,
    /// Nets hung to dry on posts (by water only).
    Nets,
}

impl Prop {
    /// The blocks it stands on, across and deep, before it is turned.
    pub fn extent(self) -> (i32, i32) {
        match self {
            Prop::Cart => (2, 3),
            Prop::LaundryLine => (5, 1),
            Prop::FenceRun => (4, 1),
            Prop::Scarecrow => (3, 1),
            Prop::Grindstone | Prop::Sacks | Prop::Washtub => (2, 1),
            Prop::ChickenCoop => (2, 2),
            Prop::Boat => (2, 4),
            Prop::Nets => (3, 1),
            _ => (1, 1),
        }
    }
}

// ---------------------------------------------------------------------------------------
// The file

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GridDef {
    pub spacing: i32,
    pub chance: f32,
    pub margin: i32,
}

impl Default for GridDef {
    fn default() -> Self {
        Self { spacing: 1024, chance: 0.33, margin: 160 }
    }
}

/// The ground a settlement may stand on, and the glade it opens in the woods.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct SiteDef {
    /// The temperatures it stands in.
    pub temperature: (f32, f32),
    /// How high over sea level its ground may lie.
    pub height: (i32, i32),
    /// The most its ground may rise and fall across its middle.
    pub relief: i32,
    /// How far it keeps from sinkholes and ravines.
    pub caves: i32,
    /// No trees within the first many blocks of a house, fewer out to the second.
    pub glade: (i32, i32),
    /// The share of trees left just past the treeless ring, and how many more where it is
    /// green.
    pub glade_trees: (f32, f32),
}

impl Default for SiteDef {
    fn default() -> Self {
        Self { temperature: (-8.0, 34.0), height: (3, 90), relief: 14, caves: 150, glade: (5, 16), glade_trees: (0.08, 0.3) }
    }
}

/// Blocks every settlement furnishes itself with whatever its style: a name each, or a
/// list (by name or tag) to pick from. Content without one leaves it out.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct FurnishingsFile {
    torch: Option<String>,
    campfire: Option<String>,
    table: Option<String>,
    farmland: Option<String>,
    water: Option<String>,
    dirt: Option<String>,
    planter: Option<String>,
    anvil: Option<String>,
    barrel: Option<String>,
    bell: Option<String>,
    altar: Option<String>,
    flowers: Vec<Select>,
    bushes: Vec<Select>,
    canopies: Vec<Select>,
    compost: Vec<Select>,
    boulders: Vec<Select>,
    fruit: Vec<Select>,
    hay: Vec<Select>,
    hanging: Vec<Select>,
    sacks: Vec<Select>,
    cloth: Vec<Select>,
}

/// [`FurnishingsFile`], its names qualified.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FurnishingsDef {
    pub torch: Option<String>,
    pub campfire: Option<String>,
    pub table: Option<String>,
    pub farmland: Option<String>,
    pub water: Option<String>,
    pub dirt: Option<String>,
    pub planter: Option<String>,
    pub anvil: Option<String>,
    pub barrel: Option<String>,
    pub bell: Option<String>,
    pub altar: Option<String>,
    pub flowers: Vec<Select>,
    pub bushes: Vec<Select>,
    pub canopies: Vec<Select>,
    pub compost: Vec<Select>,
    pub boulders: Vec<Select>,
    pub fruit: Vec<Select>,
    pub hay: Vec<Select>,
    pub hanging: Vec<Select>,
    pub sacks: Vec<Select>,
    pub cloth: Vec<Select>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeFile {
    name: String,
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    houses: (u32, u32),
    lanes: (u32, u32),
    lane_length: (u32, u32),
    square: (u32, u32),
    #[serde(default)]
    always: Vec<Special>,
    #[serde(default)]
    sometimes: Vec<(Special, f32)>,
    /// The chance it has a centrepiece.
    #[serde(default)]
    centrepiece: f32,
    /// The highest Farming level of the crops its fields grow.
    #[serde(default = "first_level")]
    crop_level: u32,
}

fn first_level() -> u32 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutFile {
    name: String,
    shape: Shape,
    weight: f32,
    /// Only where there is fresh water near.
    #[serde(default)]
    water: bool,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HouseFile {
    name: String,
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    width: (u32, u32),
    depth: (u32, u32),
    walls: (u32, u32),
    storeys: (u32, u32),
    #[serde(default)]
    porch: f32,
    #[serde(default)]
    wing: f32,
    #[serde(default)]
    form: Form,
    /// The work done in it (`bakery`, `mill`), for the people who will work there; none
    /// for a home.
    #[serde(default)]
    work: Option<String>,
    #[serde(default)]
    inside: Vec<Fitting>,
    /// Extras it always has.
    #[serde(default)]
    always: Vec<Extra>,
    /// Its own roofs, in place of the style's.
    #[serde(default)]
    roofs: Vec<(Roof, f32)>,
    /// The sizes (by name) that build it; all when empty.
    #[serde(default)]
    sizes: Vec<String>,
    /// The most of it one settlement builds.
    #[serde(default)]
    most: Option<u32>,
    /// Lifted on posts this many blocks, whatever the style.
    #[serde(default)]
    stilts: Option<u32>,
    /// Only beside fresh water.
    #[serde(default)]
    water: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoofFile {
    shape: Roof,
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CentreFile {
    /// One the code draws, or (with `blocks`) a name for one of one's own.
    name: String,
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    #[serde(default)]
    spot: Spot,
    /// Its own blocks: (x, y, z) from its spot, y 1 the block on the ground.
    #[serde(default)]
    blocks: Vec<((i32, i32, i32), String)>,
    /// The blocks a drawn one is made of, by part (`log`, `leaves`, `stone`, `worn`, `water`,
    /// `cloth`, `light`, `bell`), `{wood}` the settlement's wood; parts left out (or
    /// missing for a wood) come from the settlement's style.
    #[serde(default)]
    parts: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtraFile {
    name: Extra,
    chance: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PropFile {
    /// One of the props the code draws, or (with `blocks`) a name for a prop of one's own.
    name: String,
    weight: f32,
    /// A prop's own blocks: (x, y, z) from its spot, y 1 the block on the ground, and the
    /// block's name.
    #[serde(default)]
    blocks: Vec<((i32, i32, i32), String)>,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    /// Only on the bank of fresh water.
    #[serde(default)]
    water: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamesFile {
    #[serde(default)]
    biome: Vec<String>,
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
struct StyleFile {
    name: String,
    #[serde(default)]
    biome: Vec<String>,
    #[serde(default)]
    stilts: u32,
    woods: Vec<Select>,
    foundation: Vec<Select>,
    floor: Vec<Select>,
    walls: Vec<Select>,
    frame: Vec<Select>,
    roof: Vec<Select>,
    trim: Vec<Select>,
    roofs: Vec<(Roof, f32)>,
    window: Option<String>,
    window_chance: f32,
    paving: Vec<Select>,
    hedge: Vec<Select>,
    crops: Vec<Select>,
    #[serde(default)]
    weathered: Vec<Select>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementFile {
    grid: Option<GridDef>,
    site: Option<SiteDef>,
    #[serde(default)]
    furnishings: FurnishingsFile,
    #[serde(default)]
    avoid: Vec<String>,
    #[serde(default)]
    sizes: Vec<SizeFile>,
    #[serde(default)]
    layouts: Vec<LayoutFile>,
    #[serde(default)]
    houses: Vec<HouseFile>,
    #[serde(default)]
    extras: Vec<ExtraFile>,
    #[serde(default)]
    props: Vec<PropFile>,
    #[serde(default)]
    names: Vec<NamesFile>,
    #[serde(default)]
    styles: Vec<StyleFile>,
    #[serde(default)]
    roofs: Vec<RoofFile>,
    #[serde(default)]
    centrepieces: Vec<CentreFile>,
}

// ---------------------------------------------------------------------------------------
// Parsed definitions, names qualified by their package

#[derive(Clone, Debug, PartialEq)]
pub struct SizeDef {
    pub name: String,
    pub weight: f32,
    pub lean: Lean,
    pub houses: (u32, u32),
    pub lanes: (u32, u32),
    pub lane_length: (u32, u32),
    pub square: (u32, u32),
    pub always: Vec<Special>,
    pub sometimes: Vec<(Special, f32)>,
    pub centrepiece: f32,
    pub crop_level: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutDef {
    pub name: String,
    pub shape: Shape,
    pub weight: f32,
    pub lean: Lean,
    pub water: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HouseDef {
    pub name: String,
    pub weight: f32,
    pub lean: Lean,
    pub width: (u32, u32),
    pub depth: (u32, u32),
    pub walls: (u32, u32),
    pub storeys: (u32, u32),
    pub porch: f32,
    pub wing: f32,
    pub form: Form,
    pub work: Option<String>,
    pub inside: Vec<Fitting>,
    pub always: Vec<Extra>,
    pub roofs: Vec<(Roof, f32)>,
    pub sizes: Vec<String>,
    pub most: Option<u32>,
    pub stilts: Option<u32>,
    pub water: bool,
}

/// A roof shape every style may weigh, and the levers it leans on.
#[derive(Clone, Debug, PartialEq)]
pub struct RoofDef {
    pub shape: Roof,
    pub weight: f32,
    pub lean: Lean,
}

/// A centrepiece: drawn by the code, or a package's own blocks.
#[derive(Clone, Debug, PartialEq)]
pub enum CentreKind {
    Drawn(Centre),
    Blocks(String, Vec<(voxmar_engine::glam::IVec3, String)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CentreDef {
    pub kind: CentreKind,
    pub weight: f32,
    pub lean: Lean,
    pub spot: Spot,
    pub parts: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtraDef {
    pub name: Extra,
    pub chance: f32,
    pub lean: Lean,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropDef {
    pub kind: PropKind,
    pub weight: f32,
    pub lean: Lean,
    pub water: bool,
}

/// A prop the code draws, or one a package made of blocks.
#[derive(Clone, Debug, PartialEq)]
pub enum PropKind {
    Drawn(Prop),
    Blocks(String, Vec<(voxmar_engine::glam::IVec3, String)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamesDef {
    pub biome: Vec<String>,
    pub first: Vec<(String, String)>,
    pub last: Vec<(String, String)>,
    pub small: Vec<(String, String)>,
    /// Words put before a name ("Old", "Upper"), in English and Japanese, by weight.
    pub prefixes: Vec<(String, String, f32, Lean)>,
    pub small_chance: f32,
    pub prefix_chance: f32,
}

/// A material: block names, some with `{wood}` or `{trim}` in them, by weight.
#[derive(Clone, Debug, PartialEq)]
pub struct Material(pub Vec<(String, f32)>);

#[derive(Clone, Debug, PartialEq)]
pub struct StyleDef {
    pub name: String,
    pub biome: Vec<String>,
    pub stilts: u32,
    pub woods: Vec<(String, f32)>,
    pub foundation: Material,
    pub floor: Material,
    pub walls: Material,
    pub frame: Material,
    pub roof: Material,
    pub trim: Material,
    pub roofs: Vec<(Roof, f32)>,
    pub window: Option<String>,
    pub window_chance: f32,
    pub paving: Material,
    pub hedge: Material,
    pub crops: Vec<(String, f32)>,
    pub weathered: Material,
}

/// Every package's `settlements.ron` together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettlementsDef {
    pub grid: GridDef,
    pub site: SiteDef,
    pub furnishings: FurnishingsDef,
    pub avoid: Vec<String>,
    pub sizes: Vec<SizeDef>,
    pub layouts: Vec<LayoutDef>,
    pub houses: Vec<HouseDef>,
    pub extras: Vec<ExtraDef>,
    pub props: Vec<PropDef>,
    pub names: Vec<NamesDef>,
    pub styles: Vec<StyleDef>,
    pub roofs: Vec<RoofDef>,
    pub centrepieces: Vec<CentreDef>,
    /// Every crop's Farming level and rarity, by its full name (from `crops.ron`), so
    /// settlements grow only everyday crops a newcomer may harvest.
    pub crops: BTreeMap<String, (u32, voxmar_engine::block::Rarity)>,
}

fn names(package: &str, at: &str, selects: &[Select]) -> Result<Vec<(String, f32)>, ContentError> {
    selects
        .iter()
        .map(|s| match s {
            Select::Name(name, weight) => Ok((qualify(package, name), *weight)),
            Select::Tag(tag, _) => Err(ContentError(format!("{at}: settlements name their blocks; Tag(\"{tag}\") is not allowed"))),
        })
        .collect()
}

impl SettlementsDef {
    /// Adds a package's `settlements.ron`: its grid replaces earlier ones, its lists add
    /// to theirs.
    pub fn extend(&mut self, at: &str, package: &str, source: &str) -> Result<(), ContentError> {
        let file: SettlementFile = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(source)
            .map_err(|e| ContentError(format!("{at}: {e}")))?;
        if let Some(grid) = file.grid {
            self.grid = grid;
        }
        if let Some(site) = file.site {
            self.site = site;
        }
        // Each furnishing a later package names replaces the earlier one; lists replace
        // lists.
        let f = file.furnishings;
        let one = |mine: &mut Option<String>, theirs: Option<String>| {
            if let Some(name) = theirs {
                *mine = Some(qualify(package, &name));
            }
        };
        let mut furnishings = std::mem::take(&mut self.furnishings);
        one(&mut furnishings.torch, f.torch);
        one(&mut furnishings.campfire, f.campfire);
        one(&mut furnishings.table, f.table);
        one(&mut furnishings.farmland, f.farmland);
        one(&mut furnishings.water, f.water);
        one(&mut furnishings.dirt, f.dirt);
        one(&mut furnishings.planter, f.planter);
        one(&mut furnishings.anvil, f.anvil);
        one(&mut furnishings.barrel, f.barrel);
        one(&mut furnishings.bell, f.bell);
        one(&mut furnishings.altar, f.altar);
        let qualified = |list: Vec<Select>| -> Vec<Select> {
            list.into_iter()
                .map(|s| match s {
                    Select::Name(name, w) => Select::Name(qualify(package, &name), w),
                    tag => tag,
                })
                .collect()
        };
        for (mine, theirs) in [
            (&mut furnishings.flowers, f.flowers),
            (&mut furnishings.bushes, f.bushes),
            (&mut furnishings.canopies, f.canopies),
            (&mut furnishings.compost, f.compost),
            (&mut furnishings.boulders, f.boulders),
            (&mut furnishings.fruit, f.fruit),
            (&mut furnishings.hay, f.hay),
            (&mut furnishings.hanging, f.hanging),
            (&mut furnishings.sacks, f.sacks),
            (&mut furnishings.cloth, f.cloth),
        ] {
            if !theirs.is_empty() {
                *mine = qualified(theirs);
            }
        }
        self.furnishings = furnishings;
        self.avoid.extend(file.avoid);
        for s in file.sizes {
            self.sizes.push(SizeDef {
                lean: Lean::parse(at, &s.lean)?,
                name: s.name,
                weight: s.weight,
                houses: s.houses,
                lanes: s.lanes,
                lane_length: s.lane_length,
                square: s.square,
                always: s.always,
                sometimes: s.sometimes,
                centrepiece: s.centrepiece,
                crop_level: s.crop_level,
            });
        }
        for l in file.layouts {
            self.layouts.push(LayoutDef { lean: Lean::parse(at, &l.lean)?, name: l.name, shape: l.shape, weight: l.weight, water: l.water });
        }
        for h in file.houses {
            self.houses.push(HouseDef {
                lean: Lean::parse(at, &h.lean)?,
                name: h.name,
                weight: h.weight,
                width: h.width,
                depth: h.depth,
                walls: h.walls,
                storeys: h.storeys,
                porch: h.porch,
                wing: h.wing,
                form: h.form,
                work: h.work,
                inside: h.inside,
                always: h.always,
                roofs: h.roofs,
                sizes: h.sizes,
                most: h.most,
                stilts: h.stilts,
                water: h.water,
            });
        }
        for r in file.roofs {
            self.roofs.push(RoofDef { lean: Lean::parse(at, &r.lean)?, shape: r.shape, weight: r.weight });
        }
        for c in file.centrepieces {
            let kind = if c.blocks.is_empty() {
                let drawn = ron::from_str::<Centre>(&c.name).map_err(|_| {
                    ContentError(format!("{at}: no centrepiece is called `{}`; give it `blocks` to make one of your own", c.name))
                })?;
                CentreKind::Drawn(drawn)
            } else {
                let blocks = c.blocks.iter().map(|&((x, y, z), ref name)| (voxmar_engine::glam::IVec3::new(x, y, z), qualify(package, name))).collect();
                CentreKind::Blocks(qualify(package, &c.name), blocks)
            };
            let parts = c.parts.iter().map(|(part, name)| (part.clone(), qualify(package, name))).collect();
            self.centrepieces.push(CentreDef { lean: Lean::parse(at, &c.lean)?, kind, weight: c.weight, spot: c.spot, parts });
        }
        for e in file.extras {
            self.extras.push(ExtraDef { lean: Lean::parse(at, &e.lean)?, name: e.name, chance: e.chance });
        }
        for p in file.props {
            let kind = if p.blocks.is_empty() {
                let drawn = ron::from_str::<Prop>(&p.name).map_err(|_| {
                    ContentError(format!("{at}: no prop is called `{}`; give it `blocks` to make one of your own", p.name))
                })?;
                PropKind::Drawn(drawn)
            } else {
                let blocks = p.blocks.iter().map(|&((x, y, z), ref name)| (voxmar_engine::glam::IVec3::new(x, y, z), qualify(package, name))).collect();
                PropKind::Blocks(qualify(package, &p.name), blocks)
            };
            self.props.push(PropDef { lean: Lean::parse(at, &p.lean)?, kind, weight: p.weight, water: p.water });
        }
        for n in file.names {
            let prefixes = n
                .prefixes
                .into_iter()
                .map(|p| Ok((p.en, p.ja, p.weight, Lean::parse(at, &p.lean)?)))
                .collect::<Result<_, ContentError>>()?;
            self.names.push(NamesDef { biome: n.biome, first: n.first, last: n.last, small: n.small, prefixes, small_chance: n.small_chance, prefix_chance: n.prefix_chance });
        }
        for s in file.styles {
            let m = |list: &[Select]| names(package, at, list).map(Material);
            self.styles.push(StyleDef {
                woods: s.woods.iter().map(|w| match w {
                    Select::Name(name, weight) => Ok((name.clone(), *weight)),
                    Select::Tag(tag, _) => Err(ContentError(format!("{at}: woods are named; Tag(\"{tag}\") is not allowed"))),
                }).collect::<Result<_, _>>()?,
                foundation: m(&s.foundation)?,
                floor: m(&s.floor)?,
                walls: m(&s.walls)?,
                frame: m(&s.frame)?,
                roof: m(&s.roof)?,
                trim: m(&s.trim)?,
                paving: m(&s.paving)?,
                hedge: m(&s.hedge)?,
                weathered: m(&s.weathered)?,
                crops: names(package, at, &s.crops)?,
                window: s.window.as_deref().map(|w| qualify(package, w)),
                name: s.name,
                biome: s.biome,
                stilts: s.stilts,
                roofs: s.roofs,
                window_chance: s.window_chance,
            });
        }
        Ok(())
    }

    /// Checks the numbers and that every block named resolves, for every wood.
    pub fn check(&self, registry: &BlockRegistry) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("settlements.ron: {what}")));
        let grid = &self.grid;
        if grid.spacing < 256 || !(0.0..=1.0).contains(&grid.chance) || grid.margin < 64 || grid.margin * 2 + 64 > grid.spacing {
            return fail(format!("the grid needs a spacing of 256 or more, a chance in [0, 1] and a margin of 64 or more that leaves room: {grid:?}"));
        }
        if self.sizes.is_empty() || self.layouts.is_empty() || self.houses.is_empty() || self.styles.is_empty() || self.names.is_empty() {
            return fail("needs sizes, layouts, houses, names and styles".into());
        }
        for s in &self.sizes {
            let ordered = |(a, b): (u32, u32)| a <= b;
            if !(ordered(s.houses) && ordered(s.lanes) && ordered(s.lane_length) && ordered(s.square) && s.lanes.0 >= 1 && s.square.1 <= 16 && s.lane_length.1 <= 96) {
                return fail(format!("size `{}` needs ranges low to high, at least one lane, a square of at most 16 and lanes of at most 96", s.name));
            }
        }
        for h in &self.houses {
            if h.width.0 < 4 || h.depth.0 < 4 || h.width.1 > 12 || h.depth.1 > 16 || h.width.0 > h.width.1 || h.depth.0 > h.depth.1 || h.walls.0 < 2 || h.walls.1 > 6 || h.storeys.0 < 1 || h.storeys.1 > 3 {
                return fail(format!("house `{}` is out of bounds (width 4..=12, depth 4..=16, walls 2..=6, storeys 1..=3)", h.name));
            }
        }
        let site = &self.site;
        if site.temperature.0 > site.temperature.1 || site.height.0 > site.height.1 || site.relief < 0 || site.caves < 0 || site.glade.0 < 0 || site.glade.0 >= site.glade.1 || site.glade.1 > 48 {
            return fail(format!("the site needs ranges low to high and a glade of at most 48 blocks, its treeless ring the smaller: {site:?}"));
        }
        for c in &self.centrepieces {
            if let CentreKind::Blocks(name, blocks) = &c.kind {
                for (at, block) in blocks {
                    if at.x.abs() > 6 || at.z.abs() > 6 || !(-3..=24).contains(&at.y) {
                        return fail(format!("centrepiece `{name}` reaches past 6 blocks round its spot, 3 down or 24 up, at {at}"));
                    }
                    if registry.id(block).is_none() {
                        return fail(format!("centrepiece `{name}` names `{block}`, which is no block"));
                    }
                }
            }
        }
        for c in &self.centrepieces {
            // Parts naming `{wood}` fall back to the style where a wood lacks one.
            for (part, block) in c.parts.iter().filter(|(_, b)| !b.contains('{')) {
                if registry.id(block).is_none() {
                    return fail(format!("a centrepiece's part `{part}` names `{block}`, which is no block"));
                }
            }
        }
        for h in &self.houses {
            let round = h.form == Form::Round;
            if round && (h.width != h.depth || h.width.0 % 2 == 0 || h.width.1 % 2 == 0) {
                return fail(format!("round house `{}` needs the same odd width and depth", h.name));
            }
            if h.form == Form::Courtyard && h.width.0.min(h.depth.0) < 11 {
                return fail(format!("courtyard house `{}` needs a width and depth of 11 or more", h.name));
            }
            if h.sizes.iter().any(|s| !self.sizes.iter().any(|z| z.name == *s)) {
                return fail(format!("house `{}` names a size there is none of", h.name));
            }
        }
        for p in &self.props {
            if let PropKind::Blocks(name, blocks) = &p.kind {
                for (at, block) in blocks {
                    if at.x.abs() > 2 || at.z.abs() > 2 || !(0..=6).contains(&at.y) {
                        return fail(format!("prop `{name}` reaches past 2 blocks round its spot or 6 up, at {at}"));
                    }
                    if registry.id(block).is_none() {
                        return fail(format!("prop `{name}` names `{block}`, which is no block"));
                    }
                }
            }
        }
        for n in &self.names {
            if n.first.is_empty() || n.last.is_empty() {
                return fail("every list of names needs first and last parts".into());
            }
        }
        if !self.names.iter().any(|n| n.biome.is_empty()) || !self.styles.iter().any(|s| s.biome.is_empty()) {
            return fail("needs a style and a list of names for anywhere (no biome tags)".into());
        }
        for style in &self.styles {
            if style.woods.is_empty() || style.roofs.is_empty() || style.crops.is_empty() {
                return fail(format!("style `{}` needs woods, roofs and crops", style.name));
            }
            for (wood, _) in &style.woods {
                for (trim, _) in &style.woods {
                    for material in style.materials() {
                        for (name, _) in &material.0 {
                            let name = fill(name, wood, trim);
                            if registry.id(&name).is_none() {
                                return fail(format!("style `{}` names `{name}`, which is no block", style.name));
                            }
                        }
                    }
                }
            }
            if let Some(window) = &style.window
                && registry.id(window).is_none()
            {
                return fail(format!("style `{}`'s window `{window}` is no block", style.name));
            }
            for (crop, _) in &style.crops {
                for stage in ["_crop_0", "_crop_1", "_crop_2", "_crop"] {
                    if registry.id(&format!("{crop}{stage}")).is_none() {
                        return fail(format!("style `{}`'s crop `{crop}` has no `{crop}{stage}` block", style.name));
                    }
                }
            }
        }
        // Fields and orchards hold only everyday crops: no rarer than uncommon, no higher
        // than the largest size's crop level, one in every style a hamlet may grow, and
        // fruit trees in blossom or unripe, never ripe.
        if !self.crops.is_empty() {
            let everyday = |crop: &str, most: u32| match self.crops.get(crop) {
                None => Err(format!("`{crop}` is no crop in crops.ron")),
                Some(&(level, rarity)) if rarity > voxmar_engine::block::Rarity::Uncommon || level > most => {
                    Err(format!("`{crop}` (level {level}, {rarity:?}) is too rare or high for a settlement"))
                }
                Some(&(level, _)) => Ok(level),
            };
            let highest = self.sizes.iter().map(|s| s.crop_level).max().unwrap_or(1);
            let lowest = self.sizes.iter().map(|s| s.crop_level).min().unwrap_or(1);
            for style in &self.styles {
                let mut least = u32::MAX;
                for (crop, _) in &style.crops {
                    least = least.min(everyday(crop, highest).map_err(|e| ContentError(format!("settlements.ron: {e}")))?);
                }
                if !style.crops.is_empty() && least > lowest {
                    return fail(format!("style `{}` has no crop of level {lowest} or below for its hamlets", style.name));
                }
            }
            for select in &self.furnishings.fruit {
                let Select::Name(name, _) = select else { continue };
                let unripe = name.rsplit_once('_').filter(|(_, n)| n.parse::<u8>().is_ok());
                let Some((leaves, _)) = unripe else {
                    return fail(format!("fruit `{name}` is ripe: name a growing stage (`_1`, `_2`)"));
                };
                everyday(leaves.trim_end_matches("_leaves"), highest).map_err(|e| ContentError(format!("settlements.ron: {e}")))?;
            }
        }
        Ok(())
    }
}

impl StyleDef {
    fn materials(&self) -> [&Material; 9] {
        [&self.foundation, &self.floor, &self.walls, &self.frame, &self.roof, &self.trim, &self.paving, &self.hedge, &self.weathered]
    }
}

/// `name` with its `{wood}` and `{trim}` filled in. Woods are bare names in the package of
/// the block (`base:{wood}_log` with `oak` is `base:oak_log`).
pub fn fill(name: &str, wood: &str, trim: &str) -> String {
    name.replace("{wood}", wood).replace("{trim}", trim)
}

/// Block names to ids, for every block a style may use, made once.
#[derive(Default)]
pub struct Resolved(pub FxHashMap<String, BlockId>);

impl Resolved {
    pub fn new(def: &SettlementsDef, registry: &BlockRegistry) -> Self {
        let mut map = FxHashMap::default();
        for style in &def.styles {
            for (wood, _) in &style.woods {
                for (trim, _) in &style.woods {
                    for material in style.materials() {
                        for (name, _) in &material.0 {
                            let name = fill(name, wood, trim);
                            if let Some(id) = registry.id(&name) {
                                map.insert(name, id);
                            }
                        }
                    }
                }
            }
            for (crop, _) in &style.crops {
                for stage in ["_crop_0", "_crop_1", "_crop_2", "_crop"] {
                    let name = format!("{crop}{stage}");
                    if let Some(id) = registry.id(&name) {
                        map.insert(name, id);
                    }
                }
            }
            if let Some(window) = &style.window
                && let Some(id) = registry.id(window)
            {
                map.insert(window.clone(), id);
            }
        }
        for c in &def.centrepieces {
            if let CentreKind::Blocks(_, blocks) = &c.kind {
                for (_, name) in blocks {
                    if let Some(id) = registry.id(name) {
                        map.insert(name.clone(), id);
                    }
                }
            }
            for (_, name) in &c.parts {
                for style in &def.styles {
                    for (wood, _) in &style.woods {
                        let name = fill(name, wood, wood);
                        if let Some(id) = registry.id(&name) {
                            map.insert(name, id);
                        }
                    }
                }
            }
        }
        for p in &def.props {
            if let PropKind::Blocks(_, blocks) = &p.kind {
                for (_, name) in blocks {
                    if let Some(id) = registry.id(name) {
                        map.insert(name.clone(), id);
                    }
                }
            }
        }
        Self(map)
    }

    pub fn get(&self, name: &str) -> Option<BlockId> {
        self.0.get(name).copied()
    }
}
