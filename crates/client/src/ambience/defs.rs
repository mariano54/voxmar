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
    /// How far the middle of the nearest settlement (its square or green) is, across the
    /// ground, in blocks; [`NO_SETTLEMENT`] with none near.
    Settlement,
    /// How far outside the nearest settlement's bounds (the ground its buildings stand
    /// on, with a margin) the player is, in blocks: 0 within; [`NO_SETTLEMENT`] with none.
    SettlementEdge,
    /// How many people live in the nearest settlement (0 with none): its size.
    SettlementPeople,
}

/// The `settlement` axis with no settlement near.
pub const NO_SETTLEMENT: f32 = 10_000.0;

impl Fixed {
    pub const ALL: [Fixed; 30] = [
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
        Fixed::Settlement,
        Fixed::SettlementEdge,
        Fixed::SettlementPeople,
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
            Fixed::Settlement => "settlement",
            Fixed::SettlementEdge => "settlement_edge",
            Fixed::SettlementPeople => "settlement_people",
        }
    }
}

/// Where the player is, as numbers the layers' conditions test: the fixed axes, then one
/// per block group (0..1, how present it is around the player), and the biome's tags.
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
    pub fn new(groups: usize) -> Self {
        let mut values = vec![0.0; Fixed::ALL.len() + groups];
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
        self.ranges.iter().any(|&(a, _)| a == axis as usize)
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

/// Where a one-shot sounds from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// At a block of this group the scan found.
    Block(usize),
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

/// Every package's ambience.
#[derive(Clone, Debug, Default)]
pub struct Ambience {
    pub groups: Vec<Group>,
    /// Which blocks each group counts: tags and materials, matched against the registry,
    /// and whether only falling fluid of them (a waterfall's curtain).
    selectors: Vec<(Vec<String>, Vec<Material>, bool)>,
    pub beds: Vec<Bed>,
    pub sounds: Vec<OneShot>,
    /// The sets of buildings one-shots sound from, each once.
    pub buildings: Vec<BuildingSet>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDef {
    #[serde(default)]
    groups: Vec<GroupDef>,
    #[serde(default)]
    beds: Vec<BedDef>,
    #[serde(default)]
    sounds: Vec<SoundDef>,
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
    /// A structure's core counts only while lit.
    #[serde(default)]
    lit: bool,
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
        // Groups first, so every file's conditions may name any group.
        for (package, def) in &mut parsed {
            for group in def.groups.drain(..) {
                if ambience.groups.iter().any(|g| g.name == group.name) || Fixed::ALL.iter().any(|a| a.name() == group.name) {
                    return Err(format!("{package}/{FILE}: group {} is defined twice", group.name));
                }
                ambience.groups.push(Group { name: group.name, range: group.range, lit: group.lit });
                ambience.selectors.push((group.tags, group.materials, group.falling));
            }
        }
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
                let when = ambience.conditions(bed.when, bed.biome, bed.not_biome).map_err(|e| format!("{}: {e}", context(&bed.name)))?;
                let duck = ambience.duck(bed.duck, &bed_names, package).map_err(|e| format!("{}: {e}", context(&bed.name)))?;
                ambience.beds.push(Bed { name, sound, volume: bed.volume, when, duck, fade: bed.fade.max(0.1) });
            }
            for sound in def.sounds {
                let name = format!("{package}:{}", sound.name);
                if sound.sound.is_bed() {
                    return Err(format!("{}: {:?} is a bed, not a one-shot", context(&sound.name), sound.sound));
                }
                let at = match sound.at {
                    PlacementDef::Block(group) => Placement::Block(
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
                    reach: sound.reach.max(0.5),
                    when,
                    duck,
                });
            }
        }
        Ok(ambience)
    }

    /// The axis called `name`: a fixed one or a group.
    fn axis(&self, name: &str) -> Option<usize> {
        Fixed::ALL.iter().position(|a| a.name() == name).or_else(|| self.groups.iter().position(|g| g.name == name).map(|g| Fixed::ALL.len() + g))
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

    /// Each block's group, by block id, for the enclosure scan: the first group that
    /// selects it by tag or material.
    pub fn block_groups(&self, registry: &BlockRegistry) -> Vec<Option<u8>> {
        (0..registry.len())
            .map(|id| {
                let id = voxmar_engine::block::BlockId(id as u16);
                let (def, info) = (registry.def(id), registry.info(id));
                self.selectors
                    .iter()
                    .position(|(tags, materials, falling)| {
                        let pours = info.fluid.is_some_and(|f| f.falling);
                        (tags.iter().any(|t| def.has_tag(t)) || materials.contains(&info.sound)) && (!falling || pours)
                    })
                    .map(|g| g as u8)
            })
            .collect()
    }

    /// The environment for a group's weighted count from the scan: 0..1.
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

    pub fn presence(&self, group: usize, count: f32) -> f32 {
        let (low, high) = self.groups[group].range;
        ((count - low) / (high - low).max(1e-3)).clamp(0.0, 1.0)
    }
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

    #[test]
    fn mistakes_are_reported() {
        let error = |source: &str| Ambience::load(&file(source)).unwrap_err();
        assert!(error(r#"(beds: [(name: "x", sound: Wind, volume: 1, when: {"altitude": (min: 1)})])"#).contains("no axis altitude"));
        assert!(error(r#"(beds: [(name: "x", sound: Warbler, volume: 1)])"#).contains("not a bed"));
        assert!(error(r#"(sounds: [(name: "x", sound: Owl, volume: 1, at: Block("trees"), per_minute: 1)])"#).contains("no group trees"));
        assert!(error(r#"(beds: [(name: "x", sound: Wind, volume: 1, duck: {"thunder": 1})])"#).contains("thunder"));
    }
}
