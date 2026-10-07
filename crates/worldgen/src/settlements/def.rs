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
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct SiteDef {
    /// The temperatures it stands in at all.
    pub temperature: (f32, f32),
    /// The temperatures it is as likely in as anywhere; towards `temperature` the chance
    /// falls to `taper` of that, and to nothing past it.
    pub mild: (f32, f32),
    pub taper: f32,
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
    /// Fresh water within this many blocks of its middle (none needed at 0).
    pub water: i32,
    /// The least fertility of the ground under its buildings, on average; `poor` lowers
    /// it in biomes with a tag (the lowest of those that match).
    pub fertility: f32,
    pub poor: BTreeMap<String, f32>,
    /// Blocks every building keeps from water.
    pub bank: i32,
    /// Blocks between samples looking for a spot; how far round its middle the ground is
    /// weighed (`reach`) and must be dry and firm (`firm`); how many of the best spots
    /// are tried.
    pub step: i32,
    pub reach: i32,
    pub firm: i32,
    pub tries: usize,
    /// What a spot scores: luck, less how uneven and how steep it is, more for fresh
    /// water, less for every block over sea level.
    pub score: ScoreDef,
}

impl Default for SiteDef {
    fn default() -> Self {
        Self {
            temperature: (-10.0, 36.0),
            mild: (0.0, 26.0),
            taper: 0.25,
            height: (3, 90),
            relief: 14,
            caves: 150,
            glade: (5, 16),
            glade_trees: (0.08, 0.3),
            water: 48,
            fertility: 0.3,
            poor: BTreeMap::new(),
            bank: 3,
            step: 16,
            reach: 48,
            firm: 24,
            tries: 6,
            score: ScoreDef::default(),
        }
    }
}

impl SiteDef {
    /// How likely a settlement is at `temperature`, 0 ..= 1.
    pub fn climate(&self, temperature: f32) -> f32 {
        let ((cold, hot), (low, high)) = (self.temperature, self.mild);
        if temperature < cold || temperature > hot {
            0.0
        } else if temperature < low {
            self.taper + (1.0 - self.taper) * (temperature - cold) / (low - cold).max(1e-3)
        } else if temperature > high {
            self.taper + (1.0 - self.taper) * (hot - temperature) / (hot - high).max(1e-3)
        } else {
            1.0
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ScoreDef {
    pub luck: f32,
    pub spread: f32,
    pub relief: f32,
    pub water: f32,
    pub height: f32,
}

impl Default for ScoreDef {
    fn default() -> Self {
        Self { luck: 3.0, spread: 1.2, relief: 0.25, water: 3.0, height: 0.02 }
    }
}

/// How a lever is drawn: `luck` times the mean of `rolls` rolls, plus shares of the
/// site's fertility, of fresh water near (1 or 0), of how flat it is, of the wealth
/// already drawn, and of its forest (or undergrowth); clamped to 0 ..= 1.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LeverMix {
    pub luck: f32,
    pub rolls: u32,
    pub fertility: f32,
    pub water: f32,
    pub flat: f32,
    pub wealth: f32,
    pub forest: f32,
}

impl Default for LeverMix {
    fn default() -> Self {
        Self { luck: 1.0, rolls: 1, fertility: 0.0, water: 0.0, flat: 0.0, wealth: 0.0, forest: 0.0 }
    }
}

/// The five levers' mixes, drawn in this order (so order and crowding may follow
/// wealth); ground is flat below `steep` and undergrowth counts `undergrowth` of forest.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LeversDef {
    pub wealth: LeverMix,
    pub age: LeverMix,
    pub order: LeverMix,
    pub crowding: LeverMix,
    pub green: LeverMix,
    pub steep: f32,
    pub undergrowth: f32,
}

impl Default for LeversDef {
    fn default() -> Self {
        let mix = LeverMix::default();
        Self {
            wealth: LeverMix { luck: 0.45, fertility: 0.25, water: 0.2, flat: 0.1, ..mix },
            age: LeverMix { rolls: 2, ..mix },
            order: LeverMix { luck: 0.65, wealth: 0.35, ..mix },
            crowding: LeverMix { luck: 0.75, wealth: 0.25, ..mix },
            green: LeverMix { luck: 0.6, forest: 0.4, ..mix },
            steep: 0.6,
            undergrowth: 0.6,
        }
    }
}

/// How lanes are walked and laid out.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LanesDef {
    /// Half the width of side lanes and paths (main lanes' is the size's).
    pub side: f32,
    /// How much lanes bend: the first where orderly, the second more where not.
    pub wiggle: (f32, f32),
    /// Blocks per step walked, the most a step rises, and how far lanes keep apart.
    pub step: f32,
    pub rise: i32,
    pub apart: f32,
    /// A green's radius: the square's times the first, plus the second.
    pub green: (f32, i32),
    /// A scattered place's square.
    pub scatter: i32,
    /// How often a scattered place's path branches.
    pub branch: f32,
    /// A ring lane's distance past the square, and more where it is not crowded; how much
    /// it wobbles where disorderly; the most it rises or falls from the square.
    pub ring: (f32, f32),
    pub wobble: f32,
    pub ring_rise: i32,
    /// A terrace's step up from the square, and more where it is not crowded.
    pub terrace: (f32, f32),
    /// How far a riverside street looks for its water.
    pub riverside: i32,
}

impl Default for LanesDef {
    fn default() -> Self {
        Self {
            side: 0.8,
            wiggle: (0.06, 0.32),
            step: 3.0,
            rise: 2,
            apart: 3.5,
            green: (1.5, 2),
            scatter: 2,
            branch: 0.6,
            ring: (10.0, 4.0),
            wobble: 1.2,
            ring_rise: 6,
            terrace: (11.0, 4.0),
            riverside: 144,
        }
    }
}

/// How buildings stand along the lanes.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LotsDef {
    /// Blocks between buildings: the first, and up to the second more where not crowded.
    pub gap: (i32, f32),
    /// Blocks between buildings along a lane, and up to the second more where not crowded;
    /// each pass along the lanes tightens it by `closer`.
    pub spacing: (f32, f32),
    pub closer: f32,
    /// Blocks from a lane to a house's front, and up to the second more where not crowded.
    pub setback: (f32, f32),
    /// Yard kept clear round a building, and up to the second more where not crowded.
    pub yard: (i32, f32),
    /// The most a floor stands over or under the lane it is on.
    pub road: i32,
    /// Tries at placing each outlying building, and how many times through the lanes
    /// houses are placed.
    pub tries: u32,
    pub passes: u32,
    /// The most the ground under a house may rise and fall (more on stilts).
    pub slope: i32,
}

impl Default for LotsDef {
    fn default() -> Self {
        Self { gap: (1, 3.0), spacing: (7.0, 6.0), closer: 0.7, setback: (1.5, 3.0), yard: (1, 2.0), road: 3, tries: 10, passes: 3, slope: 4 }
    }
}

/// A special building's size, across and deep (every `step`), and the most the ground
/// under it may rise and fall.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpecialSize {
    pub width: (u32, u32),
    pub depth: (u32, u32),
    #[serde(default = "one")]
    pub step: u32,
    pub slope: i32,
}

fn one() -> u32 {
    1
}

/// What houses are made to be.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct BuildingDef {
    /// The inn: at least this many storeys, this wide and deep, from houses this wide.
    pub inn: (u32, u32, u32),
    pub grand: u32,
    /// The trader's house, across and deep.
    pub trader: ((u32, u32), (u32, u32)),
    /// Houses this wide may have a low roof, by this chance.
    pub pitch: (u32, f32),
    /// Houses line up where order passes the first; doors sit in the middle past the
    /// second.
    pub regular: f32,
    pub middle_door: f32,
    /// The style's window chance times the first, plus the second times wealth, at most
    /// the third.
    pub windows: (f32, f32, f32),
    /// The chance a house's lowest course is of its foundation stone, and old walls' share
    /// of weathered blocks.
    pub plinth: f32,
    pub moss: f32,
}

impl Default for BuildingDef {
    fn default() -> Self {
        Self { inn: (2, 8, 9), grand: 8, trader: ((5, 7), (5, 8)), pitch: (8, 0.45), regular: 0.45, middle_door: 0.6, windows: (0.6, 0.8, 0.95), plinth: 0.35, moss: 0.22 }
    }
}

/// How many props stand where.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct DressingDef {
    /// Blocks between things along a lane, and up to the second more where poorer (times
    /// the size's `lamps`).
    pub every: (f32, f32),
    /// The chance each is a lamp, and more where richer.
    pub lamp: (f32, f32),
    /// Props in yards per building: up to the first, the second more where green, and at
    /// least the third.
    pub yards: (f32, f32, u32),
    /// The most props per lane, besides those in yards.
    pub per_lane: u32,
    /// Boats and nets on the bank.
    pub banks: (u32, u32),
    /// How far props keep from lanes, by a lane and in a yard.
    pub clearance: (f32, f32),
}

impl Default for DressingDef {
    fn default() -> Self {
        Self { every: (9.0, 14.0), lamp: (0.5, 0.4), yards: (0.5, 1.0, 2), per_lane: 6, banks: (1, 3), clearance: (0.4, 0.6) }
    }
}

/// The numbers planning reads besides the site's.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct RulesDef {
    pub levers: LeversDef,
    pub lanes: LanesDef,
    pub lots: LotsDef,
    /// By kind: `smithy`, `field`, `pen`, `orchard`, `woodyard`, `shrine`, `stall`,
    /// `centrepiece` (its slope; its size is its own).
    pub specials: BTreeMap<String, SpecialSize>,
    pub houses: BuildingDef,
    pub dressing: DressingDef,
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
    /// How many working buildings it has besides its homes, and the works it always has.
    #[serde(default)]
    workshops: (u32, u32),
    #[serde(default)]
    works: Vec<String>,
    /// Half the width of its main lanes.
    #[serde(default = "lane_half")]
    lane_half: f32,
    /// The wealth past which its square, and its main lanes, are paved.
    #[serde(default = "never_paved")]
    paving: (f32, f32),
    /// Its well's width.
    #[serde(default = "well")]
    well: (u32, u32),
    /// Its lamps and props along lanes stand this much further apart.
    #[serde(default = "first_spacing")]
    lamps: f32,
}

fn lane_half() -> f32 {
    0.9
}

fn never_paved() -> (f32, f32) {
    (2.0, 2.0)
}

fn well() -> (u32, u32) {
    (3, 3)
}

fn first_spacing() -> f32 {
    1.0
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

/// How a Japanese name joins where its English runs a consonant into a vowel.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KanaJoin {
    /// The bare consonant kana a first part ends in.
    pub kana: String,
    /// The letters its English must end in too, if any ("r" for ー: Aber, not Haw).
    #[serde(default)]
    pub after: String,
    /// What it becomes before ア, イ, ウ, エ and オ.
    pub into: [String; 5],
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
    /// What chimneys are built of, one to a chimney (the first foundation stone when left
    /// out).
    #[serde(default)]
    chimney: Vec<Select>,
    /// Its logs, `{wood}` its wood (`{wood}_log` when left out).
    #[serde(default)]
    log: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementFile {
    grid: Option<GridDef>,
    site: Option<SiteDef>,
    rules: Option<RulesDef>,
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
    kana: Vec<KanaJoin>,
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
    pub workshops: (u32, u32),
    pub works: Vec<String>,
    pub lane_half: f32,
    pub paving: (f32, f32),
    pub well: (u32, u32),
    pub lamps: f32,
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
    pub chimney: Material,
    /// Its logs' block, `{wood}` its wood.
    pub log: String,
}

/// Every package's `settlements.ron` together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettlementsDef {
    pub grid: GridDef,
    pub site: SiteDef,
    pub rules: RulesDef,
    pub furnishings: FurnishingsDef,
    pub avoid: Vec<String>,
    pub sizes: Vec<SizeDef>,
    pub layouts: Vec<LayoutDef>,
    pub houses: Vec<HouseDef>,
    pub extras: Vec<ExtraDef>,
    pub props: Vec<PropDef>,
    pub names: Vec<NamesDef>,
    /// How Japanese names join where the English runs a consonant into a vowel: the bare
    /// consonant kana a first part may end in, and the five it becomes before ア, イ, ウ,
    /// エ and オ (サブ + イル: サビル, as Sab + ir is Sabir).
    pub kana: Vec<KanaJoin>,
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
        if let Some(rules) = file.rules {
            self.rules = rules;
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
        self.kana.extend(file.kana);
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
                workshops: s.workshops,
                works: s.works,
                lane_half: s.lane_half,
                paving: s.paving,
                well: s.well,
                lamps: s.lamps,
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
                chimney: m(&s.chimney)?,
                crops: names(package, at, &s.crops)?,
                window: s.window.as_deref().map(|w| qualify(package, w)),
                log: qualify(package, s.log.as_deref().unwrap_or("{wood}_log")),
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
            if !(ordered(s.workshops) && ordered(s.well) && s.well.0 >= 3 && s.well.1 <= 5 && s.lane_half > 0.0 && s.lamps > 0.0) {
                return fail(format!("size `{}` needs workshops and a well (3 to 5 wide) low to high, lanes and lamps above nothing", s.name));
            }
            for work in &s.works {
                if !self.houses.iter().any(|h| h.work.as_deref() == Some(work.as_str()) && (h.sizes.is_empty() || h.sizes.contains(&s.name))) {
                    return fail(format!("size `{}` always has a `{work}`, but no house of that work is built at its size", s.name));
                }
            }
        }
        for h in &self.houses {
            if h.width.0 < 4 || h.depth.0 < 4 || h.width.1 > 12 || h.depth.1 > 16 || h.width.0 > h.width.1 || h.depth.0 > h.depth.1 || h.walls.0 < 2 || h.walls.1 > 6 || h.storeys.0 < 1 || h.storeys.1 > 3 {
                return fail(format!("house `{}` is out of bounds (width 4..=12, depth 4..=16, walls 2..=6, storeys 1..=3)", h.name));
            }
            // Stairs run round the walls, where fittings stand.
            if !h.inside.is_empty() && h.storeys.1 > 1 {
                return fail(format!("house `{}` has fittings inside and more than one storey: its stairs would cross them", h.name));
            }
        }
        let site = &self.site;
        let within = |(a, b): (f32, f32), (lo, hi): (f32, f32)| a <= b && a >= lo && b <= hi;
        if !within(site.mild, site.temperature) || !(0.0..=1.0).contains(&site.taper) || site.step < 4 || site.firm > site.reach || site.reach < site.step || site.tries == 0 || site.bank < 0 || site.water < 0 {
            return fail(format!("the site needs mild temperatures within its temperatures, a taper in [0, 1], a step of 4 or more, a firm ground no wider than its reach, and tries: {site:?}"));
        }
        let rules = &self.rules;
        for (name, special) in &rules.specials {
            let (w, d) = (special.width, special.depth);
            if !["smithy", "field", "pen", "orchard", "woodyard", "shrine", "stall", "centrepiece"].contains(&name.as_str()) {
                return fail(format!("rules: no special building is called `{name}`"));
            }
            if w.0 > w.1 || d.0 > d.1 || w.0 < 2 || d.0 < 2 || w.1 > 16 || d.1 > 16 || special.step == 0 || special.slope < 0 {
                return fail(format!("rules: `{name}` needs a size from 2 to 16 low to high, a step and a slope: {special:?}"));
            }
        }
        if rules.lots.passes == 0 || rules.lots.tries == 0 || rules.lanes.step <= 0.0 || !(0.0..1.0).contains(&rules.lots.closer) {
            return fail("rules: lots need passes and tries, lanes a step, and `closer` in [0, 1)".into());
        }
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
        if let Some(j) = self.kana.iter().find(|j| j.kana.is_empty() || j.into.iter().any(String::is_empty)) {
            return fail(format!("kana join `{}` is empty or becomes nothing", j.kana));
        }
        if !self.names.iter().any(|n| n.biome.is_empty()) || !self.styles.iter().any(|s| s.biome.is_empty()) {
            return fail("needs a style and a list of names for anywhere (no biome tags)".into());
        }
        for style in &self.styles {
            if style.woods.is_empty() || style.roofs.is_empty() || style.crops.is_empty() {
                return fail(format!("style `{}` needs woods, roofs and crops", style.name));
            }
            for (wood, _) in &style.woods {
                let log = fill(&style.log, wood, wood);
                if registry.id(&log).is_none() {
                    return fail(format!("style `{}`'s log `{log}` is no block", style.name));
                }
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

    /// Checks every furnishing and every style's crop names blocks, so a typo fails loading
    /// rather than leaving it out. Only for whole content: the built-in base files are also
    /// read with blocks.ron alone (tools and tests), where other block files' are missing.
    pub fn check_furnishings(&self, registry: &BlockRegistry) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("settlements.ron: {what}")));
        for style in &self.styles {
            for (crop, _) in &style.crops {
                for stage in ["_crop_0", "_crop_1", "_crop_2", "_crop"] {
                    if registry.id(&format!("{crop}{stage}")).is_none() {
                        return fail(format!("style `{}`'s crop `{crop}` has no `{crop}{stage}` block", style.name));
                    }
                }
            }
        }
        let f = &self.furnishings;
        for (what, name) in [
            ("torch", &f.torch),
            ("campfire", &f.campfire),
            ("table", &f.table),
            ("farmland", &f.farmland),
            ("water", &f.water),
            ("dirt", &f.dirt),
            ("planter", &f.planter),
            ("anvil", &f.anvil),
            ("barrel", &f.barrel),
            ("bell", &f.bell),
            ("altar", &f.altar),
        ] {
            if let Some(name) = name
                && registry.id(name).is_none()
            {
                return fail(format!("furnishings: the {what} `{name}` is no block"));
            }
        }
        for (what, list) in [
            ("flowers", &f.flowers),
            ("bushes", &f.bushes),
            ("canopies", &f.canopies),
            ("compost", &f.compost),
            ("boulders", &f.boulders),
            ("fruit", &f.fruit),
            ("hay", &f.hay),
            ("hanging", &f.hanging),
            ("sacks", &f.sacks),
            ("cloth", &f.cloth),
        ] {
            for select in list {
                match select {
                    Select::Name(name, _) if registry.id(name).is_none() => return fail(format!("furnishings: {what} names `{name}`, which is no block")),
                    Select::Tag(tag, _) if registry.tagged(tag).next().is_none() => return fail(format!("furnishings: {what} names the tag `{tag}`, which no block has")),
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

impl StyleDef {
    fn materials(&self) -> [&Material; 10] {
        [&self.foundation, &self.floor, &self.walls, &self.frame, &self.roof, &self.trim, &self.paving, &self.hedge, &self.weathered, &self.chimney]
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
            for (wood, _) in &style.woods {
                let log = fill(&style.log, wood, wood);
                if let Some(id) = registry.id(&log) {
                    map.insert(log, id);
                }
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
