//! Ambience definitions, read from every content package's `ambience.ron`, so a mod adds
//! ambience the same way the base game does.
//!
//! A file holds:
//! - `groups`: kinds of block the enclosure scan counts around the player (leaves, water,
//!   rock), selected by tag or material, each with the weighted count over which it goes
//!   from absent to fully present. Each group is also an environment axis.
//! - `beds`: continuous layers, played while their conditions hold, as loud as they hold.
//! - `sounds`: one-shots, played at random at a rate per minute while their conditions
//!   hold, at a block of a group (birds in actual trees), around the player, or at the
//!   player.
//! - `roofs`: what rain sounds like on the roof overhead. Each roof material (a building
//!   material of `sounds.ron`, else the block's family sound) names a patter, or silence;
//!   each patter is an axis of its own (the rain falling, while under a roof of it), which
//!   beds play on. Under any roof the `sheltered` axis rises, by how well it shelters, for
//!   the open rain to duck under.
//!
//! Conditions are soft ranges on environment axes (AmbientSounds' approach): a layer fades
//! in over `fade` above `min` and out over `fade` above `max`, so nothing switches
//! abruptly. Layers can also want or avoid biome tags, and be ducked by an axis (rain) or
//! by another bed, which turns them down rather than off.

use std::collections::BTreeMap;

use serde::Deserialize;
use voxmar_engine::block::{BlockRegistry, Sound as Material};
use voxmar_engine::content::ContentFile;

use super::synth::Synth;

/// The file ambience is defined in, in any content package.
pub const FILE: &str = "ambience.ron";

/// An environment axis with a fixed meaning; block groups add one axis each after these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixed {
    /// How much air the player is in, 0 (a cupboard) to 1 (open land).
    Air,
    /// How much of the nearby air is open to the sky, 0..1.
    Sky,
    /// How much of the sky-open air nearby is under leaves, 0..1.
    Canopy,
    /// How enclosed and echoing the place is: 0 outdoors, 1 in a large cave.
    Reverb,
    /// Height of the sun, -1 (midnight) to 1 (noon); around 0 at dawn and dusk.
    Sun,
    /// Air temperature at the surface here, °C.
    Temperature,
    /// The climate's moisture: about 0.1 in deserts, 1 balanced, 2 in rainforests.
    Moisture,
    /// -1 over deep ocean, 0 at the coast, 1 deep inland.
    Continentalness,
    /// 1 in a river or lake, falling to 0 at the edge of its valley.
    River,
    /// As `River`, but only by running water (creeks and rivers), and only by still water
    /// (lakes and springs).
    Flowing,
    Still,
    /// Tree cover of the climate, 0..1.
    Forest,
    /// The player's height above sea level, in blocks.
    Height,
    /// The player's height above the land around, in blocks: high on peaks and ridges.
    Relief,
    /// How far below the ground surface here the player is, in blocks.
    Depth,
    /// Rain heard falling: 0..1, less under a roof.
    Rain,
    /// Rain falling at all, roof or not, 0..1: for the sound of it on the roof overhead.
    Raining,
    /// How wet the ground is, 0..1: dripping trees after the rain.
    Wetness,
    /// Snow falling: 0..1.
    Snow,
    /// Thunderstorm: rain from heaped cloud, 0..1.
    Storm,
    /// Wind speed, blocks per second, gusts included.
    Wind,
    /// How much the wind gusts, 0..1.
    Gusts,
    /// Hail and sleet falling where the player can hear them, 0..1.
    Hail,
    Sleet,
    /// Sand or dust blowing, 0..1.
    Dust,
    /// Cloud cover, 0..1.
    Cover,
    /// 1 with the head under water.
    Underwater,
    /// How indoors the player is, 0..1: a roof overhead and walls around, a door open or
    /// not. Outdoor layers duck by it, and one-shots that do are muffled as much.
    Shelter,
    /// A roof close overhead that the sky falls on, 0..1, walls or not: rain drums on it.
    Roof,
    /// How far the middle of the nearest settlement (its square or green) is, across the
    /// ground, in blocks; [`NO_SETTLEMENT`] with none near.
    Settlement,
    /// How much a roof overhead muffles the open rain, 0 (none overhead) to 1 (`roofs`).
    Sheltered,
    /// How far outside the nearest settlement's bounds (the ground its buildings stand
    /// on, with a margin) the player is, in blocks: 0 within; [`NO_SETTLEMENT`] with none.
    SettlementEdge,
    /// How many people live in the nearest settlement (0 with none): its size.
    SettlementPeople,
    /// How far up the home ladder the player stands, by the highest of `home`'s effects
    /// they hold: 0 away from home, 1 for the first, and so on.
    Home,
}

/// The `settlement` axis with no settlement near.
pub const NO_SETTLEMENT: f32 = 10_000.0;

impl Fixed {
    pub const ALL: [Fixed; 34] = [
        Fixed::Air,
        Fixed::Sky,
        Fixed::Canopy,
        Fixed::Reverb,
        Fixed::Sun,
        Fixed::Temperature,
        Fixed::Moisture,
        Fixed::Continentalness,
        Fixed::River,
        Fixed::Flowing,
        Fixed::Still,
        Fixed::Forest,
        Fixed::Height,
        Fixed::Relief,
        Fixed::Depth,
        Fixed::Rain,
        Fixed::Raining,
        Fixed::Wetness,
        Fixed::Snow,
        Fixed::Storm,
        Fixed::Wind,
        Fixed::Gusts,
        Fixed::Hail,
        Fixed::Sleet,
        Fixed::Dust,
        Fixed::Cover,
        Fixed::Underwater,
        Fixed::Shelter,
        Fixed::Roof,
        Fixed::Settlement,
        Fixed::Sheltered,
        Fixed::SettlementEdge,
        Fixed::SettlementPeople,
        Fixed::Home,
    ];

    fn name(self) -> &'static str {
        match self {
            Fixed::Air => "air",
            Fixed::Sky => "sky",
            Fixed::Canopy => "canopy",
            Fixed::Reverb => "reverb",
            Fixed::Sun => "sun",
            Fixed::Temperature => "temperature",
            Fixed::Moisture => "moisture",
            Fixed::Continentalness => "continentalness",
            Fixed::River => "river",
            Fixed::Forest => "forest",
            Fixed::Height => "height",
            Fixed::Relief => "relief",
            Fixed::Depth => "depth",
            Fixed::Flowing => "flowing",
            Fixed::Still => "still",
            Fixed::Rain => "rain",
            Fixed::Raining => "raining",
            Fixed::Wetness => "wetness",
            Fixed::Snow => "snow",
            Fixed::Storm => "storm",
            Fixed::Wind => "wind",
            Fixed::Gusts => "gusts",
            Fixed::Hail => "hail",
            Fixed::Sleet => "sleet",
            Fixed::Dust => "dust",
            Fixed::Cover => "cover",
            Fixed::Underwater => "underwater",
            Fixed::Shelter => "shelter",
            Fixed::Roof => "roof",
            Fixed::Settlement => "settlement",
            Fixed::Sheltered => "sheltered",
            Fixed::SettlementEdge => "settlement_edge",
            Fixed::SettlementPeople => "settlement_people",
            Fixed::Home => "home",
        }
    }
}

/// Where the player is, as numbers the layers' conditions test: the fixed axes, then one
/// per block group (0..1, how present it is around the player), then one per roof patter
/// (the rain falling on a roof of it overhead), and the biome's tags.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    pub values: Vec<f32>,
    /// Tags of the biome here, of the neighbouring one, and how much the place belongs to
    /// the first (1 deep inside, 0.5 at the border).
    pub home_tags: Vec<String>,
    pub neighbour_tags: Vec<String>,
    pub home: f32,
}

impl Environment {
    /// With `axes` axes after the fixed ones: the groups', then the roof patters'.
    pub fn new(axes: usize) -> Self {
        let mut values = vec![0.0; Fixed::ALL.len() + axes];
        values[Fixed::Settlement as usize] = NO_SETTLEMENT;
        values[Fixed::SettlementEdge as usize] = NO_SETTLEMENT;
        Self { values, home: 1.0, ..Self::default() }
    }

    pub fn get(&self, axis: Fixed) -> f32 {
        self.values[axis as usize]
    }

    pub fn set(&mut self, axis: Fixed, value: f32) {
        self.values[axis as usize] = value;
    }

    pub fn set_group(&mut self, group: usize, value: f32) {
        self.values[Fixed::ALL.len() + group] = value;
    }

    /// How much the place has a biome tag, 0..1, blended across a biome border.
    pub fn tag(&self, tag: &str) -> f32 {
        let has = |tags: &[String]| tags.iter().any(|t| t == tag) as u8 as f32;
        self.home * has(&self.home_tags) + (1.0 - self.home) * has(&self.neighbour_tags)
    }
}

/// A soft range on an axis: full between `min` and `max`, fading to nothing over `fade`
/// below `min` and above `max`. Either may be left out.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub fade: f32,
}

impl Default for Range {
    fn default() -> Self {
        Self { min: f32::NEG_INFINITY, max: f32::INFINITY, fade: 0.0 }
    }
}

impl Range {
    pub fn weight(&self, x: f32) -> f32 {
        let ramp = |d: f32| {
            if self.fade <= 0.0 {
                return (d >= self.fade) as u8 as f32;
            }
            let t = (d / self.fade).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let above = if self.min.is_finite() { ramp(x - self.min + self.fade) } else { 1.0 };
        let below = if self.max.is_finite() { ramp(self.max + self.fade - x) } else { 1.0 };
        above * below
    }
}

/// What turns a layer down: an axis such as rain, or another bed, and by how much at its
/// fullest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DuckBy {
    Axis(usize),
    Bed(usize),
}

#[derive(Clone, Debug, Default)]
pub struct Conditions {
    ranges: Vec<(usize, Range)>,
    biome: Vec<String>,
    not_biome: Vec<String>,
}

impl Conditions {
    /// Whether they hold a range on `axis`.
    pub fn uses(&self, axis: Fixed) -> bool {
        self.uses_axis(axis as usize)
    }

    /// Whether they hold a range on the axis numbered `axis` (a group's or a patter's too).
    pub fn uses_axis(&self, axis: usize) -> bool {
        self.ranges.iter().any(|&(a, _)| a == axis)
    }

    /// How fully the conditions hold, 0..1.
    pub fn weight(&self, env: &Environment) -> f32 {
        let mut weight = self.ranges.iter().map(|&(axis, range)| range.weight(env.values[axis])).product::<f32>();
        if !self.biome.is_empty() {
            weight *= self.biome.iter().map(|t| env.tag(t)).fold(0.0, f32::max);
        }
        if !self.not_biome.is_empty() {
            weight *= 1.0 - self.not_biome.iter().map(|t| env.tag(t)).fold(0.0, f32::max);
        }
        weight
    }
}

/// A kind of block the enclosure scan counts.
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    /// Weighted count where the group starts to be present, and where it is fully present.
    pub range: (f32, f32),
    /// Whether a structure's core (a hearthstone, a crucible) counts only while its fire
    /// burns (`ClientWorld::fires`); the group's other blocks always count.
    pub lit: bool,
    /// Whether its blocks count only while they smoulder (`ClientWorld::smoulders`): thatch
    /// about to catch, heard crackling before it does.
    pub smouldering: bool,
    /// The lit group whose cores it counts once their fire has gone out, and for how many
    /// seconds after: a kiln's bricks ticking as they cool. It selects nothing itself.
    pub cools: Option<(usize, f32)>,
}

/// What a bed plays: a synthesized sound, or recorded segments shipped in its package.
#[derive(Clone, Debug, PartialEq)]
pub enum BedSound {
    Synth(Synth),
    /// Asset paths of [`synth::BED_VARIANTS`](super::synth::BED_VARIANTS) segments, each
    /// [`synth::SEGMENT`](super::synth::SEGMENT) seconds long and fading in and out with
    /// equal power over the first and last [`synth::OVERLAP`](super::synth::OVERLAP)
    /// seconds, as the synthesized ones do.
    Recorded(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct Bed {
    pub name: String,
    pub sound: BedSound,
    pub volume: f32,
    pub when: Conditions,
    pub duck: Vec<(DuckBy, f32)>,
    /// Seconds to fade fully in or out.
    pub fade: f32,
}

impl Bed {
    /// An outdoor layer, heard muffled from indoors: one that ducks by `shelter`.
    pub fn outdoor(&self) -> bool {
        self.duck.iter().any(|(by, _)| *by == DuckBy::Axis(Fixed::Shelter as usize))
    }
}

/// Where a one-shot sounds from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// At a block of this group the scan found.
    Block(usize),
    /// At a block of this group with nothing over it but the sky (rain falls on it); none
    /// such, not heard.
    OpenBlock(usize),
    /// In a random direction this far away (blocks) and this high above the player.
    Around { distance: (f32, f32), height: (f32, f32) },
    /// At the player, from no direction.
    Listener,
    /// At one of the nearest settlement's buildings near (`BUILDINGS_HEARD`) that answer to
    /// a name of the set (`Ambience::buildings`): its door, or its middle. None near, none
    /// heard.
    Building(usize),
}

/// Buildings a one-shot sounds from (`Building` in ambience.ron): those answering to any
/// of `names` (a kind, a house, a work: `voxmar_worldgen::SettlementBuilding::answers`),
/// heard at the `door` or else from the middle.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildingSet {
    pub names: Vec<String>,
    pub door: bool,
}

#[derive(Clone, Debug)]
pub struct OneShot {
    pub name: String,
    pub synth: Synth,
    pub volume: f32,
    pub at: Placement,
    pub per_minute: f32,
    /// Seconds after playing before it may play again.
    pub cooldown: f32,
    /// Range of playback speed, which also shifts the pitch.
    pub pitch: (f32, f32),
    /// Distance at which it is half as loud as up close, in blocks.
    pub reach: f32,
    pub when: Conditions,
    pub duck: Vec<(DuckBy, f32)>,
}

/// How being indoors is told and heard (a file's `shelter`; a later package's replaces it).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shelter {
    /// Blocks out from the eye a wall may be and still count.
    pub walls: i32,
    /// Blocks above the eye a roof may be for rain to be heard drumming on it.
    pub roof: i32,
    /// Hz an outdoor one-shot is muffled down to when fully indoors: a one-shot is outdoor
    /// when it ducks by `shelter`, and muffled as far as it ducks.
    pub muffle: f32,
    /// How loud thunder is fully indoors, against out in the open.
    pub thunder: f32,
}

impl Default for Shelter {
    fn default() -> Self {
        Self { walls: 6, roof: 8, muffle: OPEN_AIR_HZ, thunder: 1.0 }
    }
}

/// A cutoff that leaves a sound as it is.
pub const OPEN_AIR_HZ: f32 = 20_000.0;

/// What rain sounds like on a roof (`roofs`).
#[derive(Clone, Debug, Default)]
pub struct Roofs {
    /// Blocks above the player's head within which a roof is looked for.
    pub height: i32,
    /// How much any roof overhead muffles the open rain (the `sheltered` axis), 0..1.
    pub muffle: f32,
    /// How much more a roof block with each shelter tag muffles it, of what is left.
    pub shelter: Vec<(String, f32)>,
    /// How much more a roof of each patter muffles it, of what is left (by `patters`'
    /// index): thatch hushes, stone and brick shut it out. None for a roof with one of the
    /// `draughty` tags (a lean-to open at its sides).
    pub patter_shelter: Vec<f32>,
    pub draughty: Vec<String>,
    /// Each roof material's patter (an index into `patters`), or silence.
    pub patter: Vec<(String, Option<usize>)>,
    /// The patters, each an axis.
    pub patters: Vec<String>,
}

impl Roofs {
    /// The `sheltered` axis under a roof block with `tags` that patters `patter`: the
    /// muffle, and more for what it is made of unless draughty. Each block muffles the rain
    /// once: by what it is made of when that patter muffles, else by its shelter tags (the
    /// walls' tags are the shelter flood's, for the wind and the draught).
    pub fn sheltered(&self, has_tag: impl Fn(&str) -> bool, patter: Option<usize>) -> f32 {
        let draughty = self.draughty.iter().any(|tag| has_tag(tag));
        let made = patter.filter(|_| !draughty).and_then(|p| self.patter_shelter.get(p)).copied().filter(|&more| more > 0.0);
        let left = match made {
            Some(more) => (1.0 - self.muffle) * (1.0 - more),
            None => self.shelter.iter().filter(|(tag, _)| has_tag(tag)).fold(1.0 - self.muffle, |left, (_, more)| left * (1.0 - more)),
        };
        (1.0 - left).clamp(0.0, 1.0)
    }
}

/// Every package's ambience.
#[derive(Clone, Debug, Default)]
pub struct Ambience {
    pub shelter: Shelter,
    pub groups: Vec<Group>,
    /// Which blocks each group counts: tags and materials, matched against the registry,
    /// and whether only falling fluid of them (a waterfall's curtain).
    selectors: Vec<(Vec<String>, Vec<Material>, bool, Option<voxmar_engine::effect::Shelter>)>,
    pub beds: Vec<Bed>,
    pub sounds: Vec<OneShot>,
    pub roofs: Roofs,
    /// Each draughty shelter tag and the beds its walls whistle with (`draughts`), each
    /// bed playing on a group that counts that tag.
    pub draughts: Vec<(String, Vec<usize>)>,
    /// The sets of buildings one-shots sound from, each once.
    pub buildings: Vec<BuildingSet>,
    /// The home ladder's effects, lowest first (`home`): the `home` axis is how far up it
    /// the player stands.
    pub home: Vec<String>,
    /// The effects beds and sounds hear held (`effect:NAME`), each an axis after the
    /// patters': 1 while the player holds it. The server's room survey, heard as it judged.
    pub held: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDef {
    #[serde(default)]
    shelter: Option<Shelter>,
    #[serde(default)]
    groups: Vec<GroupDef>,
    #[serde(default)]
    beds: Vec<BedDef>,
    #[serde(default)]
    sounds: Vec<SoundDef>,
    #[serde(default)]
    roofs: Option<RoofsDef>,
    #[serde(default)]
    draughts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    home: Option<Vec<String>>,
}

/// `roofs`: a later package's height and muffle replace an earlier one's; its shelter and
/// patter entries replace those of the same name and add to the rest.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoofsDef {
    #[serde(default)]
    height: Option<i32>,
    #[serde(default)]
    muffle: Option<f32>,
    #[serde(default)]
    shelter: BTreeMap<String, f32>,
    #[serde(default)]
    patter: BTreeMap<String, Option<String>>,
    #[serde(default)]
    patter_shelter: BTreeMap<String, f32>,
    #[serde(default)]
    draughty: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupDef {
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    materials: Vec<Material>,
    /// Only falling fluid (water pouring down: a waterfall).
    #[serde(default)]
    falling: bool,
    range: (f32, f32),
    /// Its blocks count only while they smoulder.
    #[serde(default)]
    smouldering: bool,
    /// A structure's core counts only while lit.
    #[serde(default)]
    lit: bool,
    /// The cores of this lit group, for these seconds after their fire goes out.
    #[serde(default)]
    cools: Option<(String, f32)>,
    /// Walls of this shelter, as a home's room is judged (`Shelter::of_wall`: by the
    /// block's tag, a solid untagged block draughty, furniture none), and never the floor.
    #[serde(default)]
    shelter: Option<voxmar_engine::effect::Shelter>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BedDef {
    name: String,
    /// A synthesized sound, or else `recording`: segment files in the package.
    #[serde(default)]
    sound: Option<Synth>,
    #[serde(default)]
    recording: Vec<String>,
    volume: f32,
    #[serde(default)]
    when: BTreeMap<String, Range>,
    #[serde(default)]
    biome: Vec<String>,
    #[serde(default)]
    not_biome: Vec<String>,
    #[serde(default)]
    duck: BTreeMap<String, f32>,
    #[serde(default = "default_fade")]
    fade: f32,
}

fn default_fade() -> f32 {
    4.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum PlacementDef {
    Block(String),
    OpenBlock(String),
    Around { distance: (f32, f32), height: (f32, f32) },
    Listener,
    Building {
        buildings: Vec<String>,
        #[serde(default)]
        door: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SoundDef {
    name: String,
    sound: Synth,
    volume: f32,
    at: PlacementDef,
    per_minute: f32,
    #[serde(default)]
    cooldown: f32,
    #[serde(default = "default_pitch")]
    pitch: (f32, f32),
    #[serde(default = "default_reach")]
    reach: f32,
    #[serde(default)]
    when: BTreeMap<String, Range>,
    #[serde(default)]
    biome: Vec<String>,
    #[serde(default)]
    not_biome: Vec<String>,
    #[serde(default)]
    duck: BTreeMap<String, f32>,
}

fn default_pitch() -> (f32, f32) {
    (0.95, 1.05)
}

/// Sets each effect heard held (`held`, from [`Ambience::held_effects`]) to 1 if the
/// player holds it, else 0.
pub fn set_held(env: &mut Environment, ambience: &Ambience, held: &[Option<voxmar_engine::effect::EffectId>], holding: &[voxmar_engine::effect::ActiveEffect]) {
    for (i, effect) in held.iter().enumerate() {
        let on = effect.is_some_and(|id| holding.iter().any(|a| a.effect == id));
        env.values[ambience.held_axis(i)] = on as u8 as f32;
    }
}

/// How far up the home `ladder` (see [`Ambience::home_ladder`]) a player holding `held`
/// stands: 1 + the place of the highest step they hold, 0 for none.
pub fn home_step(ladder: &[voxmar_engine::effect::EffectId], held: &[voxmar_engine::effect::ActiveEffect]) -> f32 {
    ladder.iter().rposition(|&id| held.iter().any(|a| a.effect == id)).map_or(0.0, |step| step as f32 + 1.0)
}

/// The loudest a bed or a one-shot may be (`volume`), and the farthest a one-shot may
/// carry at half loudness (`reach`, blocks): as creature sounds are bounded, so that no
/// package deafens the player or fills the world.
pub const VOLUME_MOST: f32 = 2.0;
pub const REACH_MOST: f32 = 128.0;
/// The longest a group may count a fire gone out (`cools`), in seconds.
pub const COOLS_MOST: f32 = 600.0;

fn default_reach() -> f32 {
    12.0
}

impl Ambience {
    /// Reads every package's `ambience.ron`, in package order. Beds and sounds are named
    /// `package:name`; groups keep their plain names, as they are axes.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut parsed = Vec::new();
        for file in files.iter().filter(|f| f.name == FILE) {
            let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
            let def: FileDef = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            parsed.push((file.package.as_str(), def));
        }
        let mut ambience = Ambience::default();
        for (package, def) in &parsed {
            if let Some(shelter) = def.shelter {
                let sane = shelter.walls >= 1 && shelter.roof >= 1 && (20.0..=OPEN_AIR_HZ).contains(&shelter.muffle) && (0.0..=1.0).contains(&shelter.thunder);
                if !sane {
                    return Err(format!("{package}/{FILE}: shelter wants walls and roof of a block or more, muffle 20..20000 Hz, thunder 0..1"));
                }
                ambience.shelter = shelter;
            }
            if let Some(home) = &def.home {
                ambience.home.clone_from(home);
            }
        }
        // Groups first, so every file's conditions may name any group.
        let mut cools = Vec::new();
        for (package, def) in &mut parsed {
            for group in def.groups.drain(..) {
                if ambience.groups.iter().any(|g| g.name == group.name) || Fixed::ALL.iter().any(|a| a.name() == group.name) {
                    return Err(format!("{package}/{FILE}: group {} is defined twice", group.name));
                }
                if group.shelter.is_some() && (!group.tags.is_empty() || !group.materials.is_empty() || group.falling || group.lit || group.smouldering) {
                    return Err(format!("{package}/{FILE}: group {} selects walls by shelter, and nothing else", group.name));
                }
                if group.cools.is_some() && (!group.tags.is_empty() || !group.materials.is_empty() || group.falling || group.lit || group.smouldering || group.shelter.is_some()) {
                    return Err(format!("{package}/{FILE}: group {} counts cooling cores, and nothing else", group.name));
                }
                cools.push(group.cools.map(|(of, seconds)| (package.to_string(), of, seconds)));
                ambience.groups.push(Group { name: group.name, range: group.range, lit: group.lit, smouldering: group.smouldering, cools: None });
                ambience.selectors.push((group.tags, group.materials, group.falling, group.shelter));
            }
        }
        // Each cooling group names a lit group, any file's.
        for (index, cools) in cools.into_iter().enumerate() {
            let Some((package, of, seconds)) = cools else { continue };
            let name = &ambience.groups[index].name;
            let Some(lit) = ambience.groups.iter().position(|g| g.name == of && g.lit) else {
                return Err(format!("{package}/{FILE}: group {name} cools {of}, which is no lit group"));
            };
            if !(seconds > 0.0 && seconds <= COOLS_MOST) {
                return Err(format!("{package}/{FILE}: group {name} cools for {seconds} s, not over 0 to {COOLS_MOST}"));
            }
            if let Some(other) = ambience.groups.iter().find(|g| g.cools.is_some_and(|(cooled, _)| cooled == lit)) {
                return Err(format!("{package}/{FILE}: group {name} cools {of}, which {} already cools", other.name));
            }
            ambience.groups[index].cools = Some((lit, seconds));
        }
        // Roofs next, as each patter is an axis.
        let (mut shelter, mut patter, mut patter_shelter) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        for (package, def) in &mut parsed {
            let Some(roofs) = def.roofs.take() else { continue };
            if let Some(height) = roofs.height {
                if !(1..=64).contains(&height) {
                    return Err(format!("{package}/{FILE}: roofs: height {height} is not 1 to 64 blocks"));
                }
                ambience.roofs.height = height;
            }
            if let Some(muffle) = roofs.muffle {
                ambience.roofs.muffle = muffle.clamp(0.0, 1.0);
            }
            shelter.extend(roofs.shelter.into_iter().map(|(tag, more)| (tag, more.clamp(0.0, 1.0))));
            patter.extend(roofs.patter);
            patter_shelter.extend(roofs.patter_shelter.into_iter().map(|(kind, more)| (kind, more.clamp(0.0, 1.0))));
            if let Some(draughty) = roofs.draughty {
                ambience.roofs.draughty = draughty;
            }
        }
        let mut patters: Vec<String> = patter.values().flatten().cloned().collect();
        patters.sort();
        patters.dedup();
        for name in &patters {
            if ambience.axis(name).is_some() {
                return Err(format!("{FILE}: roof patter {name} is already an axis"));
            }
        }
        ambience.roofs.patter = patter.into_iter().map(|(material, kind)| (material, kind.map(|k| patters.iter().position(|p| *p == k).expect("listed")))).collect();
        if let Some(kind) = patter_shelter.keys().find(|kind| !patters.contains(kind)) {
            return Err(format!("{FILE}: roofs: patter_shelter names {kind}, which no roof material patters"));
        }
        ambience.roofs.patter_shelter = patters.iter().map(|p| patter_shelter.get(p).copied().unwrap_or(0.0)).collect();
        ambience.roofs.patters = patters;
        ambience.roofs.shelter = shelter.into_iter().collect();
        let draughts: Vec<(String, BTreeMap<String, Vec<String>>)> = parsed.iter_mut().map(|(package, def)| (package.to_string(), std::mem::take(&mut def.draughts))).collect();
        // Effects held next, as each `effect:NAME` a bed or sound names is an axis.
        let keys = |when: &BTreeMap<String, Range>, duck: &BTreeMap<String, f32>| when.keys().chain(duck.keys()).filter_map(|k| k.strip_prefix("effect:")).map(str::to_owned).collect::<Vec<_>>();
        let mut held: Vec<String> = parsed
            .iter()
            .flat_map(|(_, def)| def.beds.iter().flat_map(|b| keys(&b.when, &b.duck)).chain(def.sounds.iter().flat_map(|s| keys(&s.when, &s.duck))))
            .collect();
        held.sort();
        held.dedup();
        ambience.held = held;
        // Beds next, so sounds and beds can be ducked by any bed.
        let bed_names: Vec<String> =
            parsed.iter().flat_map(|(package, def)| def.beds.iter().map(move |b| format!("{package}:{}", b.name))).collect();
        for (package, def) in parsed {
            let context = |name: &str| format!("{package}/{FILE}: {name}");
            for bed in def.beds {
                let name = format!("{package}:{}", bed.name);
                let sound = match (bed.sound, bed.recording.is_empty()) {
                    (Some(synth), true) if synth.is_bed() => BedSound::Synth(synth),
                    (Some(synth), true) => return Err(format!("{}: {synth:?} is a one-shot sound, not a bed", context(&bed.name))),
                    (None, false) if bed.recording.len() == super::synth::BED_VARIANTS => {
                        BedSound::Recorded(bed.recording.iter().map(|path| format!("content/{package}/{path}")).collect())
                    }
                    (None, false) => {
                        return Err(format!("{}: a recording needs {} segments", context(&bed.name), super::synth::BED_VARIANTS));
                    }
                    _ => return Err(format!("{}: give a sound or a recording, not both", context(&bed.name))),
                };
                if !(0.0..=VOLUME_MOST).contains(&bed.volume) {
                    return Err(format!("{}: volume {} is not 0 to {VOLUME_MOST}", context(&bed.name), bed.volume));
                }
                let when = ambience.conditions(bed.when, bed.biome, bed.not_biome).map_err(|e| format!("{}: {e}", context(&bed.name)))?;
                let duck = ambience.duck(bed.duck, &bed_names, package).map_err(|e| format!("{}: {e}", context(&bed.name)))?;
                ambience.beds.push(Bed { name, sound, volume: bed.volume, when, duck, fade: bed.fade.max(0.1) });
            }
            for sound in def.sounds {
                let name = format!("{package}:{}", sound.name);
                if sound.sound.is_bed() {
                    return Err(format!("{}: {:?} is a bed, not a one-shot", context(&sound.name), sound.sound));
                }
                if !(0.0..=VOLUME_MOST).contains(&sound.volume) || !(1.0..=REACH_MOST).contains(&sound.reach) {
                    return Err(format!("{}: volume {} (0 to {VOLUME_MOST}) and reach {} (1 to {REACH_MOST} blocks)", context(&sound.name), sound.volume, sound.reach));
                }
                let at = match sound.at {
                    PlacementDef::Block(group) => Placement::Block(
                        ambience.groups.iter().position(|g| g.name == group).ok_or_else(|| format!("{}: no group {group}", context(&sound.name)))?,
                    ),
                    PlacementDef::OpenBlock(group) => Placement::OpenBlock(
                        ambience.groups.iter().position(|g| g.name == group).ok_or_else(|| format!("{}: no group {group}", context(&sound.name)))?,
                    ),
                    PlacementDef::Around { distance, height } => Placement::Around { distance, height },
                    PlacementDef::Listener => Placement::Listener,
                    PlacementDef::Building { buildings, door } => {
                        if buildings.is_empty() {
                            return Err(format!("{}: sounds from no building", context(&sound.name)));
                        }
                        let set = BuildingSet { names: buildings, door };
                        Placement::Building(ambience.buildings.iter().position(|s| *s == set).unwrap_or_else(|| {
                            ambience.buildings.push(set);
                            ambience.buildings.len() - 1
                        }))
                    }
                };
                let when =
                    ambience.conditions(sound.when, sound.biome, sound.not_biome).map_err(|e| format!("{}: {e}", context(&sound.name)))?;
                let duck = ambience.duck(sound.duck, &bed_names, package).map_err(|e| format!("{}: {e}", context(&sound.name)))?;
                ambience.sounds.push(OneShot {
                    name,
                    synth: sound.sound,
                    volume: sound.volume,
                    at,
                    per_minute: sound.per_minute,
                    cooldown: sound.cooldown,
                    pitch: sound.pitch,
                    reach: sound.reach,
                    when,
                    duck,
                });
            }
        }
        // Draughts last, naming beds: a later package's entry for a tag replaces an earlier
        // one's. Each bed must play on a group that counts the tag, so more of those walls
        // near is louder and none is silence.
        let mut resolved: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (package, entries) in draughts {
            for (tag, names) in entries {
                let mut beds = Vec::new();
                for name in names {
                    let local = format!("{package}:{name}");
                    let bed = bed_names.iter().position(|b| *b == name || *b == local).ok_or_else(|| format!("{package}/{FILE}: draughts: {tag} whistles with {name}, which is no bed"))?;
                    let selects = |g: usize| ambience.selectors[g].0.contains(&tag) || ambience.selectors[g].3.is_some_and(|s| s.tag() == tag);
                    let counts = (0..ambience.groups.len()).any(|g| selects(g) && ambience.beds[bed].when.uses_axis(Fixed::ALL.len() + g));
                    if !counts {
                        return Err(format!("{package}/{FILE}: draughts: {name} plays on no group counting {tag}, so it would not grow with the draughty walls near"));
                    }
                    beds.push(bed);
                }
                resolved.insert(tag, beds);
            }
        }
        ambience.draughts = resolved.into_iter().collect();
        Ok(ambience)
    }

    /// Fails, for the launch, on a `home` step that names no effect, or names one twice, or
    /// an `effect:NAME` heard held that is no effect.
    pub fn check_home(&self, effects: &voxmar_engine::effect::EffectRegistry) -> Result<(), String> {
        if let Some(name) = self.held.iter().find(|name| effects.find(name).is_none()) {
            return Err(format!("{FILE}: effect:{name} is heard held, but {name} is no effect"));
        }
        for (i, name) in self.home.iter().enumerate() {
            if effects.find(name).is_none() {
                return Err(format!("{FILE}: home names {name}, which is no effect"));
            }
            if self.home[..i].contains(name) {
                return Err(format!("{FILE}: home names {name} twice"));
            }
        }
        Ok(())
    }

    /// The home ladder's effects, lowest first, looked up once (a name that is no effect,
    /// which the launch refuses, is no step).
    pub fn home_ladder(&self, effects: &voxmar_engine::effect::EffectRegistry) -> Vec<voxmar_engine::effect::EffectId> {
        self.home.iter().filter_map(|name| effects.find(name)).collect()
    }

    /// The effects heard held (`held`), looked up once, in order (one that is no effect,
    /// which the launch refuses, is never held).
    pub fn held_effects(&self, effects: &voxmar_engine::effect::EffectRegistry) -> Vec<Option<voxmar_engine::effect::EffectId>> {
        self.held.iter().map(|name| effects.find(name)).collect()
    }

    /// Fails, for the launch, on a block's `shelter:` tag that neither keeps the rain's
    /// noise out (`roofs.shelter`) nor whistles (`draughts`): every grade of shelter a
    /// block can have says how it sounds.
    pub fn check_shelter_tags(&self, registry: &BlockRegistry) -> Result<(), String> {
        for id in 0..registry.len() {
            let def = registry.def(voxmar_engine::block::BlockId(id as u16));
            for tag in def.tags.iter().filter(|t| t.starts_with("shelter:")) {
                let heard = self.roofs.shelter.iter().any(|(t, _)| t == tag) || self.draughts.iter().any(|(t, _)| t == tag);
                if !heard {
                    return Err(format!("{FILE}: {} is {tag}, which neither roofs.shelter muffles by nor draughts whistles with", def.name));
                }
            }
        }
        Ok(())
    }

    /// The axis called `name`: a fixed one, a group or a roof patter.
    fn axis(&self, name: &str) -> Option<usize> {
        Fixed::ALL
            .iter()
            .position(|a| a.name() == name)
            .or_else(|| self.groups.iter().position(|g| g.name == name).map(|g| Fixed::ALL.len() + g))
            .or_else(|| self.roofs.patters.iter().position(|p| p == name).map(|p| self.patter_axis(p)))
            .or_else(|| name.strip_prefix("effect:").and_then(|e| self.held.iter().position(|h| h == e)).map(|i| self.held_axis(i)))
    }

    /// The axes after the fixed ones: the groups', the roof patters' and the effects held.
    pub fn axes(&self) -> usize {
        self.groups.len() + self.roofs.patters.len() + self.held.len()
    }

    /// The axis of the `i`th effect heard held.
    pub fn held_axis(&self, i: usize) -> usize {
        self.patter_axis(self.roofs.patters.len()) + i
    }

    /// The axis of roof patter `patter`.
    pub fn patter_axis(&self, patter: usize) -> usize {
        Fixed::ALL.len() + self.groups.len() + patter
    }

    /// Each block's roof patter, by block id: none for a block that is no roof (one sound
    /// and rain pass through: air, plants, leaves) or whose material is silent. `material`
    /// names a block's roof material: its building material, else its family sound
    /// (`stone`). Fails, for the launch, on a roof whose material has no patter entry, an
    /// entry naming no material there is, or a patter no bed plays on.
    pub fn roof_patters(&self, registry: &BlockRegistry, materials: &[&str], material: impl Fn(voxmar_engine::block::BlockId) -> String) -> Result<Vec<Option<u8>>, String> {
        let known = |name: &str| materials.contains(&name) || Material::ALL.iter().any(|m| family(*m) == name);
        if let Some((name, _)) = self.roofs.patter.iter().find(|(name, _)| !known(name)) {
            return Err(format!("{FILE}: roofs: patter for {name}, which is no material"));
        }
        for (p, name) in self.roofs.patters.iter().enumerate() {
            let axis = self.patter_axis(p);
            if !self.beds.iter().any(|bed| bed.when.ranges.iter().any(|&(a, _)| a == axis)) {
                return Err(format!("{FILE}: roofs: no bed plays on the {name} patter"));
            }
        }
        (0..registry.len())
            .map(|id| {
                let id = voxmar_engine::block::BlockId(id as u16);
                if !is_roof(registry, id) {
                    return Ok(None);
                }
                let name = material(id);
                match self.roofs.patter.iter().find(|(m, _)| *m == name) {
                    Some((_, patter)) => Ok(patter.map(|p| p as u8)),
                    None => Err(format!("{FILE}: roofs: no patter (or None, for silence) for {name}, the material of {}", registry.def(id).name)),
                }
            })
            .collect()
    }

    fn conditions(&self, when: BTreeMap<String, Range>, biome: Vec<String>, not_biome: Vec<String>) -> Result<Conditions, String> {
        let ranges = when.into_iter().map(|(name, range)| Ok((self.axis(&name).ok_or_else(|| format!("no axis {name}"))?, range))).collect::<Result<_, String>>()?;
        Ok(Conditions { ranges, biome, not_biome })
    }

    /// Ducking by an axis, or by a bed named `package:name` (or just `name` in the same
    /// package).
    fn duck(&self, duck: BTreeMap<String, f32>, beds: &[String], package: &str) -> Result<Vec<(DuckBy, f32)>, String> {
        duck.into_iter()
            .map(|(by, amount)| {
                let local = format!("{package}:{by}");
                let target = if let Some(axis) = self.axis(&by) {
                    DuckBy::Axis(axis)
                } else if let Some(bed) = beds.iter().position(|b| *b == by || *b == local) {
                    DuckBy::Bed(bed)
                } else {
                    return Err(format!("nothing called {by} to duck under"));
                };
                Ok((target, amount.clamp(0.0, 1.0)))
            })
            .collect()
    }

    /// Each block's wall group, by block id, for the enclosure scan: the first group
    /// selecting by `shelter` whose shelter the block has as a wall (`walls`, by block id:
    /// as the server judges a home's room, `voxmar_server::wall_shelters`). Apart from its
    /// own group, so earth is both the ground a meadow sounds from and a draughty wall.
    pub fn wall_groups(&self, walls: &[Option<voxmar_engine::effect::Shelter>]) -> Vec<Option<u8>> {
        walls.iter().map(|&wall| self.selectors.iter().position(|selector| selector.3.is_some() && selector.3 == wall).map(|g| g as u8)).collect()
    }

    /// Each block's group, by block id, for the enclosure scan: the first group that
    /// selects it by tag or material (a group selecting by shelter is a wall group).
    pub fn block_groups(&self, registry: &BlockRegistry) -> Vec<Option<u8>> {
        // (A cooling group selects nothing: no tags, no materials.)
        (0..registry.len())
            .map(|id| {
                let id = voxmar_engine::block::BlockId(id as u16);
                let (def, info) = (registry.def(id), registry.info(id));
                self.selectors
                    .iter()
                    .position(|(tags, materials, falling, shelter)| {
                        let pours = info.fluid.is_some_and(|f| f.falling);
                        shelter.is_none() && (tags.iter().any(|t| def.has_tag(t)) || materials.contains(&info.sound)) && (!falling || pours)
                    })
                    .map(|g| g as u8)
            })
            .collect()
    }

    /// By group, for the enclosure scan: where a lit group's cores count once their fire
    /// has gone out, and for how long (`Kinds::cooling`).
    pub fn cooling(&self) -> Vec<Option<(u8, f32)>> {
        let mut cooling = vec![None; self.groups.len()];
        for (index, group) in self.groups.iter().enumerate() {
            if let Some((lit, seconds)) = group.cools {
                cooling[lit] = Some((index as u8, seconds));
            }
        }
        cooling
    }

    /// The longest any group counts a fire gone out: how long one is worth remembering.
    pub fn cools_longest(&self) -> f32 {
        self.groups.iter().filter_map(|g| g.cools.map(|c| c.1)).fold(0.0, f32::max)
    }

    /// Whether every recorded bed's files are there under `assets`, so a mistyped path
    /// fails at launch rather than leaving the bed silent.
    pub fn check_recordings(&self, assets: &std::path::Path) -> Result<(), String> {
        for bed in &self.beds {
            if let BedSound::Recorded(paths) = &bed.sound
                && let Some(missing) = paths.iter().find(|path| !assets.join(path).is_file())
            {
                return Err(format!("{FILE}: bed `{}` is recorded in `{missing}`, which is not there", bed.name));
            }
        }
        Ok(())
    }

    /// Whether every building a one-shot sounds from is one settlements have (`known`:
    /// `SettlementsDef::building_names`), so a mistyped name is not a sound never heard.
    pub fn check_buildings(&self, known: &std::collections::BTreeSet<String>) -> Result<(), String> {
        for sound in &self.sounds {
            let Placement::Building(set) = sound.at else { continue };
            if let Some(name) = self.buildings[set].names.iter().find(|name| !known.contains(*name)) {
                return Err(format!("{FILE}: {} sounds from building {name}, which no settlement has", sound.name));
            }
        }
        Ok(())
    }

    /// The environment for a group's weighted count from the scan: 0..1.
    pub fn presence(&self, group: usize, count: f32) -> f32 {
        let (low, high) = self.groups[group].range;
        ((count - low) / (high - low).max(1e-3)).clamp(0.0, 1.0)
    }
}

/// A family sound's name as roofs name it: `stone`, `wood`.
pub fn family(sound: Material) -> String {
    format!("{sound:?}").to_lowercase()
}

/// Whether rain on `id` overhead is rain on a roof: it is solid and not leaves (rain falls
/// through leaves, and the canopy has its own sound).
pub fn is_roof(registry: &BlockRegistry, id: voxmar_engine::block::BlockId) -> bool {
    let info = registry.info(id);
    info.solid && info.render != voxmar_engine::block::RenderKind::Leaves
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_ranges_fade_in_and_out() {
        let range = Range { min: 10.0, max: 20.0, fade: 5.0 };
        assert_eq!(range.weight(4.0), 0.0);
        assert_eq!(range.weight(5.0), 0.0);
        assert!((range.weight(7.5) - 0.5).abs() < 1e-6);
        assert_eq!(range.weight(10.0), 1.0);
        assert_eq!(range.weight(20.0), 1.0);
        assert!((range.weight(22.5) - 0.5).abs() < 1e-6);
        assert_eq!(range.weight(26.0), 0.0);
        let step = Range { min: 0.0, ..Range::default() };
        assert_eq!((step.weight(-0.1), step.weight(0.0)), (0.0, 1.0));
    }

    fn file(source: &str) -> Vec<ContentFile> {
        vec![ContentFile { package: "test".into(), name: FILE.into(), source: source.into() }]
    }

    #[test]
    fn definitions_resolve_axes_groups_and_ducking() {
        let ambience = Ambience::load(&file(
            r#"(
                groups: [(name: "leaves", tags: ["leaves"], range: (10, 100))],
                beds: [
                    (name: "gale", sound: Gale, volume: 0.3, when: {"relief": (min: 10, fade: 20)}),
                    (name: "rustle", sound: Rustle, volume: 0.2, when: {"leaves": (min: 0.1, fade: 0.2)}, duck: {"rain": 0.6, "gale": 0.5}),
                ],
                sounds: [(name: "bird", sound: Warbler, volume: 0.3, at: Block("leaves"), per_minute: 6, biome: ["forest"])],
            )"#,
        ))
        .unwrap();
        assert_eq!(ambience.beds[1].name, "test:rustle");
        assert_eq!(ambience.beds[1].duck, vec![(DuckBy::Bed(0), 0.5), (DuckBy::Axis(Fixed::Rain as usize), 0.6)]);
        assert_eq!(ambience.sounds[0].at, Placement::Block(0));

        let mut env = Environment::new(1);
        env.set_group(0, 1.0);
        assert_eq!(ambience.beds[1].when.weight(&env), 1.0);
        // The bird wants a forest; at a border with one, it half belongs.
        assert_eq!(ambience.sounds[0].when.weight(&env), 0.0);
        env.neighbour_tags = vec!["forest".into()];
        env.home = 0.5;
        assert_eq!(ambience.sounds[0].when.weight(&env), 0.5);
    }

    /// A cooling group names a lit group, for a while, once, and selects nothing else.
    #[test]
    fn a_cooling_group_cools_one_lit_group() {
        let lit = r#"(name: "kiln", tags: ["crucible"], range: (1, 4), lit: true)"#;
        let groups = |more: &str| format!("(groups: [{lit}, (name: \"rock\", tags: [\"stone\"], range: (1, 4)), {more}])");
        let ambience = Ambience::load(&file(&groups(r#"(name: "cooling", cools: ("kiln", 90), range: (1, 4))"#))).unwrap();
        assert_eq!(ambience.groups[2].cools, Some((0, 90.0)));
        assert_eq!(ambience.cooling(), vec![Some((2, 90.0)), None, None]);
        assert_eq!(ambience.cools_longest(), 90.0);
        let error = |more: &str| Ambience::load(&file(&groups(more))).unwrap_err();
        assert!(error(r#"(name: "cooling", cools: ("rock", 90), range: (1, 4))"#).contains("no lit group"));
        assert!(error(r#"(name: "cooling", cools: ("kiln", 0), range: (1, 4))"#).contains("not over 0"));
        assert!(error(r#"(name: "cooling", cools: ("kiln", 601), range: (1, 4))"#).contains("not over 0"));
        assert!(error(r#"(name: "cooling", cools: ("kiln", 90), tags: ["brick"], range: (1, 4))"#).contains("nothing else"));
        assert!(error(r#"(name: "a", cools: ("kiln", 90), range: (1, 4)), (name: "b", cools: ("kiln", 30), range: (1, 4))"#).contains("already cools"));
    }

    #[test]
    fn mistakes_are_reported() {
        let error = |source: &str| Ambience::load(&file(source)).unwrap_err();
        assert!(error(r#"(beds: [(name: "x", sound: Wind, volume: 1, when: {"altitude": (min: 1)})])"#).contains("no axis altitude"));
        assert!(error(r#"(beds: [(name: "x", sound: Warbler, volume: 1)])"#).contains("not a bed"));
        assert!(error(r#"(sounds: [(name: "x", sound: Owl, volume: 1, at: Block("trees"), per_minute: 1)])"#).contains("no group trees"));
        assert!(error(r#"(sounds: [(name: "x", sound: Owl, volume: 1, at: OpenBlock("trees"), per_minute: 1)])"#).contains("no group trees"));
        assert!(error(r#"(beds: [(name: "x", sound: Wind, volume: 1, duck: {"thunder": 1})])"#).contains("thunder"));
        // Too loud, too far, or not a number: refused at load.
        for bad in [
            r#"(beds: [(name: "x", sound: Wind, volume: 2.5)])"#,
            r#"(beds: [(name: "x", sound: Wind, volume: NaN)])"#,
            r#"(sounds: [(name: "x", sound: Owl, volume: 3, at: Listener, per_minute: 1)])"#,
            r#"(sounds: [(name: "x", sound: Owl, volume: 0.3, at: Listener, per_minute: 1, reach: 500)])"#,
            r#"(sounds: [(name: "x", sound: Owl, volume: 0.3, at: Listener, per_minute: 1, reach: 0.5)])"#,
        ] {
            assert!(error(bad).contains("volume"), "{bad}");
        }
        assert!(Ambience::load(&file(r#"(beds: [(name: "x", sound: Wind, volume: 2.0)], sounds: [(name: "y", sound: Owl, volume: 0.3, at: Listener, per_minute: 1, reach: 128)])"#)).is_ok());
        assert!(error(r#"(shelter: (muffle: 5))"#).contains("shelter"));
        assert!(error(r#"(shelter: (walls: 0))"#).contains("shelter"));
        assert!(error(r#"(shelter: (thunder: 1.5))"#).contains("shelter"));
    }

    /// A one-shot can sound from a settlement's buildings by name, at their doors or from
    /// their middles; sets asked for twice are one, a set of no names is a mistake, and the
    /// names must be buildings settlements have.
    #[test]
    fn building_sounds_resolve_and_are_checked() {
        let ambience = Ambience::load(&file(
            r#"(sounds: [
                (name: "anvil", sound: Hammer, volume: 0.3, at: Building(buildings: ["smithy"]), per_minute: 2, cooldown: 3),
                (name: "door", sound: Door, volume: 0.3, at: Building(buildings: ["house"], door: true), per_minute: 2, cooldown: 3),
                (name: "again", sound: Hammer, volume: 0.3, at: Building(buildings: ["smithy"]), per_minute: 2, cooldown: 3),
            ])"#,
        ))
        .unwrap();
        assert_eq!(ambience.buildings, [BuildingSet { names: vec!["smithy".into()], door: false }, BuildingSet { names: vec!["house".into()], door: true }]);
        assert_eq!(ambience.sounds.iter().map(|s| s.at).collect::<Vec<_>>(), [Placement::Building(0), Placement::Building(1), Placement::Building(0)]);
        let known: std::collections::BTreeSet<String> = ["smithy", "house"].map(String::from).into();
        assert_eq!(ambience.check_buildings(&known), Ok(()));
        let unknown = ambience.check_buildings(&["house".to_owned()].into());
        assert!(unknown.as_ref().is_err_and(|e| e.contains("smithy")), "{unknown:?}");
        let none = Ambience::load(&file(r#"(sounds: [(name: "x", sound: Hammer, volume: 0.3, at: Building(buildings: []), per_minute: 2, cooldown: 3)])"#));
        assert!(none.as_ref().is_err_and(|e| e.contains("no building")), "{:?}", none.map(|_| ()));
        // The shipped sounds name buildings the shipped settlements have.
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let world = voxmar_worldgen::WorldContent::load(&content).unwrap();
        let shipped = Ambience::load(&content.files).unwrap();
        assert!(!shipped.buildings.is_empty());
        assert_eq!(shipped.check_buildings(&world.settlements.building_names()), Ok(()));
    }

    /// Roof patters are axes beds play on; a later package's entries replace an earlier
    /// one's; a roof material with no entry, an entry naming no material, or a patter no
    /// bed plays on fails the launch check.
    #[test]
    fn roof_patters_resolve_and_are_checked() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        let files = |extra: &str| {
            let mut all = file(
                r#"(
                    roofs: (height: 10, muffle: 0.4, shelter: {"shelter:fine": 0.5}, patter: {"stone": "dull", "wood": "tap", "glass": "tap"}, patter_shelter: {"dull": 0.5}, draughty: ["open"]),
                    beds: [(name: "drum", sound: Hail, volume: 0.3, when: {"dull": (min: 0.3)}), (name: "tick", sound: Sleet, volume: 0.3, when: {"tap": (min: 0.3)})],
                )"#,
            );
            all.push(ContentFile { package: "mod".into(), name: FILE.into(), source: extra.into() });
            all
        };
        let ambience = Ambience::load(&files(r#"(roofs: (patter: {"wood": "dull"}))"#)).unwrap();
        assert_eq!(ambience.roofs.patters, ["dull", "tap"]);
        assert_eq!((ambience.roofs.height, ambience.roofs.muffle), (10, 0.4));
        assert!(ambience.beds[0].when.uses_axis(ambience.patter_axis(0)));
        assert!((ambience.roofs.sheltered(|t| t == "shelter:fine", None) - 0.7).abs() < 1e-6 && (ambience.roofs.sheltered(|_| false, None) - 0.4).abs() < 1e-6);
        // What the roof is made of muffles too: a dull (stone) roof more, unless draughty.
        assert!((ambience.roofs.sheltered(|_| false, Some(0)) - 0.7).abs() < 1e-6, "{}", ambience.roofs.sheltered(|_| false, Some(0)));
        assert!((ambience.roofs.sheltered(|t| t == "open", Some(0)) - 0.4).abs() < 1e-6 && (ambience.roofs.sheltered(|_| false, Some(1)) - 0.4).abs() < 1e-6);
        assert!(Ambience::load(&files(r#"(roofs: (patter_shelter: {"thud": 0.2}))"#)).unwrap_err().contains("thud"));
        let family = |id| family(blocks.info(id).sound);
        let listed: Vec<String> = ambience.roofs.patter.iter().map(|(m, _)| m.clone()).collect();
        // Every other family a roof can be made of silent, for this test.
        let quiet: String = Material::ALL.iter().map(|m| super::family(*m)).filter(|m| !listed.contains(m)).map(|m| format!("\"{m}\": None, ")).collect();
        let ambience = Ambience::load(&files(&format!(r#"(roofs: (patter: {{"wood": "dull", {quiet}}}))"#))).unwrap();
        let patters = ambience.roof_patters(blocks, &[], family).unwrap();
        let of = |name: &str| patters[blocks.id(name).unwrap().0 as usize];
        assert_eq!((of("base:stone"), of("base:oak_planks"), of("base:glass"), of("core:air")), (Some(0), Some(0), Some(1), None));
        let error = |extra: &str| Ambience::load(&files(extra)).unwrap().roof_patters(blocks, &[], family).unwrap_err();
        assert!(error("()").contains("no patter"), "a roof material with no entry");
        assert!(error(&format!(r#"(roofs: (patter: {{"stoen": "dull", {quiet}}}))"#)).contains("stoen"));
        assert!(error(&format!(r#"(roofs: (patter: {{{quiet}"sand": "hiss"}}))"#)).contains("hiss"), "a patter no bed plays on");
        assert!(Ambience::load(&files(r#"(roofs: (patter: {"sand": "rain"}))"#)).unwrap_err().contains("already an axis"));
    }

    /// A draughty tag names beds playing on a group that counts it, and every `shelter:` tag
    /// a block has must muffle the rain or whistle: the launch fails on one that does
    /// neither.
    #[test]
    fn draughts_are_checked() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let load = |draughts: &str| {
            Ambience::load(&file(&format!(
                r#"(
                    groups: [(name: "draughty", tags: ["shelter:draughty"], range: (1, 10)), (name: "wood", tags: ["log"], range: (1, 10))],
                    beds: [(name: "whistle", sound: Draught, volume: 0.2, when: {{"draughty": (min: 1)}}), (name: "creak", sound: Draught, volume: 0.2, when: {{"wood": (min: 1)}})],
                    roofs: (shelter: {{"shelter:sound": 0.3, "shelter:fine": 0.5}}),
                    draughts: {draughts},
                )"#
            )))
        };
        let fine = load(r#"{"shelter:draughty": ["whistle"]}"#).unwrap();
        assert_eq!(fine.draughts, [("shelter:draughty".to_owned(), vec![0])]);
        fine.check_shelter_tags(&content.blocks).unwrap();
        assert!(load(r#"{"shelter:draughty": ["howl"]}"#).unwrap_err().contains("howl, which is no bed"));
        assert!(load(r#"{"shelter:draughty": ["creak"]}"#).unwrap_err().contains("no group counting shelter:draughty"));
        // A group may count walls by their shelter, as the home judges them, and by nothing
        // else beside.
        let walls = |extra: &str| {
            Ambience::load(&file(&format!(
                r#"(groups: [(name: "walls", shelter: Draughty, range: (1, 10){extra})], beds: [(name: "whistle", sound: Draught, volume: 0.2, when: {{"walls": (min: 1)}})], draughts: {{"shelter:draughty": ["whistle"]}})"#
            )))
        };
        let by_shelter = walls("").unwrap();
        let wall_groups = by_shelter.wall_groups(&voxmar_server::wall_shelters(&content));
        let of = |name: &str| wall_groups[content.blocks.id(name).unwrap().0 as usize];
        assert_eq!((of("base:wattle_panel"), of("base:dirt"), of("base:daub"), of("core:air")), (Some(0), Some(0), None, None), "wattle and bare earth are draughty walls; daub and air are not");
        assert!(by_shelter.block_groups(&content.blocks).iter().all(Option::is_none), "a wall group is apart from the blocks' own groups");
        assert!(walls(r#", tags: ["log"]"#).unwrap_err().contains("by shelter, and nothing else"));
        // Wattle is draughty: with no whistle for it, the launch fails naming it.
        let error = load("{}").unwrap().check_shelter_tags(&content.blocks).unwrap_err();
        assert!(error.contains("shelter:draughty"), "{error}");
    }
}
