//! Home stations, as content (`stations.ron`, docs/stations.md): blocks a player loads and
//! leaves to work while they are away, a drying rack or a compost heap. Each is three
//! blocks: `empty`, `working` (something on it) and `ready` (done, to be taken). A right
//! click with something it works puts a load on (up to `holds`); one on a ready station
//! takes what it made. A load is done `minutes` after it went on, on the world's clock,
//! whether anyone is near or not: `faster` rules about the spot when it was loaded
//! (a fire near a drying rack) shorten it, and `needs` (open sky over a rack) are asked
//! when it is loaded and when it is taken.
//!
//! Beside the stations, `fertilisers`: an item held to a growing crop grows the crops
//! within its reach a stage (compost from the heap; a bone, a single crop).

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError, Sound};
use crate::skill::{SkillId, SkillRegistry};

/// Something about the spot a station stands on.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Spot {
    /// Nothing but air above it, up to the sky.
    OpenSky,
    /// A block with this tag within this many blocks (any way, diagonals too).
    Near(String, i32),
    /// Water (any fluid but a hot one, or a block that waters: a well) within this many blocks.
    Water(i32),
}

/// One thing a station works: `takes` of `input` become `gives` of `output`, `minutes`
/// after they went on. The input is an item, `tag:TAG`, items with every one of several
/// tags (`tag:raw+meat`) and none of those marked `!` (`tag:grain+!vegetable`), or any of
/// several of these (`tag:fruit|wheat_seeds`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkDef {
    pub input: String,
    #[serde(default = "one")]
    pub takes: u8,
    pub output: String,
    #[serde(default = "one")]
    pub gives: u8,
    pub minutes: f32,
    /// The skill it asks a level in to load, and trains when what it made is taken
    /// (`xp` for each one taken).
    pub skill: String,
    #[serde(default = "one")]
    pub level: u8,
    #[serde(default)]
    pub xp: f32,
    /// What quests count what it made as when it is taken: `cooked` (as if cooked at a
    /// fire, for `Cook` goals) or `crafted` (`Craft` goals). None counts it as neither.
    #[serde(default)]
    pub counts: Option<String>,
}

/// The quest tallies a work's output may count toward (`WorkDef::counts`).
pub const COUNTS: [&str; 2] = ["cooked", "crafted"];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationDef {
    pub empty: String,
    pub working: String,
    pub ready: String,
    /// The most a load may be.
    pub holds: u8,
    pub works: Vec<WorkDef>,
    /// What the spot must be, when loaded and when taken.
    #[serde(default)]
    pub needs: Vec<Spot>,
    /// What about the spot, when loaded, makes the work go this many times as fast (all
    /// that hold, multiplied).
    #[serde(default)]
    pub faster: Vec<(Spot, f32)>,
    /// What loading it sounds like: a material's stroke (`Water`: something stirred in),
    /// else its working block's placing.
    #[serde(default)]
    pub sound: Option<Sound>,
    /// Whether, loaded in the loader's home, it tops the load up from the home's stores (the
    /// pack's own first): true unless said otherwise.
    #[serde(default = "yes")]
    pub from_stores: bool,
}

/// An item held to a growing crop grows the crops within `reach` of it `stages` stages.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FertiliserDef {
    pub item: String,
    pub reach: f32,
    #[serde(default = "one")]
    pub stages: u8,
    #[serde(default = "farming")]
    pub skill: String,
    #[serde(default)]
    pub xp: f32,
}

fn one() -> u8 {
    1
}

fn yes() -> bool {
    true
}

fn farming() -> String {
    "core:farming".to_owned()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationFile {
    #[serde(default)]
    pub stations: Vec<StationDef>,
    #[serde(default)]
    pub fertilisers: Vec<FertiliserDef>,
}

/// A work, resolved: `accepts` is sorted.
#[derive(Clone, Debug)]
pub struct Work {
    pub accepts: Vec<BlockId>,
    pub takes: u8,
    pub output: BlockId,
    pub gives: u8,
    pub minutes: f32,
    pub skill: SkillId,
    pub level: u8,
    pub xp: f32,
    pub counts: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Station {
    pub empty: BlockId,
    pub working: BlockId,
    pub ready: BlockId,
    pub holds: u8,
    pub works: Vec<Work>,
    pub needs: Vec<Spot>,
    pub faster: Vec<(Spot, f32)>,
    pub sound: Option<Sound>,
    pub from_stores: bool,
}

impl Station {
    /// What it does with `item`, if it works it.
    pub fn work(&self, item: BlockId) -> Option<(usize, &Work)> {
        self.works.iter().enumerate().find(|(_, w)| w.accepts.binary_search(&item).is_ok())
    }
}

#[derive(Clone, Debug)]
pub struct Fertiliser {
    pub item: BlockId,
    pub reach: f32,
    pub stages: u8,
    pub skill: SkillId,
    pub xp: f32,
}

/// Where a station block stands in its work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Empty,
    Working,
    Ready,
}

#[derive(Clone, Debug, Default)]
pub struct Stations {
    pub stations: Vec<Station>,
    pub fertilisers: Vec<Fertiliser>,
}

impl Stations {
    pub fn read(namespace: &str, source: &str) -> Result<StationFile, RegistryError> {
        let mut file: StationFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/stations.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        for station in &mut file.stations {
            for name in [&mut station.empty, &mut station.working, &mut station.ready] {
                qualify(name);
            }
            for work in &mut station.works {
                work.input = work.input.split('|').map(|one| if one.starts_with("tag:") || one.contains(':') { one.to_owned() } else { format!("{namespace}:{one}") }).collect::<Vec<_>>().join("|");
                qualify(&mut work.output);
                qualify(&mut work.skill);
            }
        }
        for fertiliser in &mut file.fertilisers {
            qualify(&mut fertiliser.item);
            qualify(&mut fertiliser.skill);
        }
        Ok(file)
    }

    /// Every package's stations and fertilisers, checked: each block belongs to one
    /// station, each item is an item, and the times, loads, reaches and paces make sense.
    pub fn new(files: Vec<StationFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let refuse = |what: String| RegistryError(format!("stations.ron: {what}"));
        let block = |name: &str| blocks.id(name).ok_or_else(|| refuse(format!("no block `{name}`")));
        let skill = |name: &str| skills.id(name).ok_or_else(|| refuse(format!("no skill `{name}`")));
        let spot = |s: &Spot| match s {
            Spot::OpenSky => Ok(()),
            Spot::Near(tag, reach) if (1..=8).contains(reach) && blocks.ids().any(|id| blocks.def(id).has_tag(tag)) => Ok(()),
            Spot::Near(tag, reach) => Err(refuse(format!("near `{tag}` within {reach}: the reach is 1 to 8, and some block must have the tag"))),
            Spot::Water(reach) if (1..=8).contains(reach) => Ok(()),
            Spot::Water(reach) => Err(refuse(format!("water within {reach}: the reach is 1 to 8"))),
        };
        let mut stations = Stations::default();
        for file in files {
            for def in file.stations {
                let (empty, working, ready) = (block(&def.empty)?, block(&def.working)?, block(&def.ready)?);
                if empty == working || working == ready || empty == ready {
                    return Err(refuse(format!("{}: its empty, working and ready blocks must differ", def.empty)));
                }
                if stations.stations.iter().any(|s| [s.empty, s.working, s.ready].iter().any(|b| [empty, working, ready].contains(b))) {
                    return Err(refuse(format!("{}: a block belongs to two stations", def.empty)));
                }
                if def.works.is_empty() || def.holds == 0 || def.holds > 64 {
                    return Err(refuse(format!("{}: a station works something and holds 1 to 64", def.empty)));
                }
                def.needs.iter().try_for_each(spot)?;
                for (s, times) in &def.faster {
                    spot(s)?;
                    if !times.is_finite() || *times <= 1.0 || *times > 8.0 {
                        return Err(refuse(format!("{}: faster by {times}: above 1, at most 8", def.empty)));
                    }
                }
                let mut works = Vec::new();
                for work in &def.works {
                    let mut accepts: Vec<BlockId> = Vec::new();
                    for one in work.input.split('|') {
                        match one.strip_prefix("tag:") {
                            Some(tags) => {
                                let has = |id: BlockId, tag: &str| match tag.strip_prefix('!') {
                                    Some(not) => !blocks.def(id).has_tag(not),
                                    None => blocks.def(id).has_tag(tag),
                                };
                                let tagged: Vec<BlockId> = blocks.ids().filter(|&id| tags.split('+').all(|tag| has(id, tag))).collect();
                                if tagged.is_empty() {
                                    return Err(refuse(format!("{}: no item has the tag in `{one}`", def.empty)));
                                }
                                accepts.extend(tagged);
                            }
                            None => accepts.push(block(one)?),
                        }
                    }
                    accepts.sort();
                    accepts.dedup();
                    let output = block(&work.output)?;
                    if work.takes == 0 || work.gives == 0 || work.takes > def.holds {
                        return Err(refuse(format!("{}: {} takes {} for {}, and a load holds {}", def.empty, work.input, work.takes, work.gives, def.holds)));
                    }
                    if !work.minutes.is_finite() || work.minutes <= 0.0 || !work.xp.is_finite() || work.xp < 0.0 || work.level == 0 {
                        return Err(refuse(format!("{}: {} takes minutes above 0, a level from 1, and experience of 0 or more", def.empty, work.input)));
                    }
                    if accepts.contains(&output) {
                        return Err(refuse(format!("{}: {} works into itself", def.empty, work.output)));
                    }
                    if let Some(both) = works.iter().flat_map(|w: &Work| w.accepts.iter()).find(|a| accepts.binary_search(a).is_ok()) {
                        return Err(refuse(format!("{}: {} has two works", def.empty, blocks.def(*both).name)));
                    }
                    if let Some(kind) = work.counts.as_deref().filter(|kind| !COUNTS.contains(kind)) {
                        return Err(refuse(format!("{}: {} counts as `{kind}`, not one of {COUNTS:?}", def.empty, work.output)));
                    }
                    works.push(Work { accepts, takes: work.takes, output, gives: work.gives, minutes: work.minutes, skill: skill(&work.skill)?, level: work.level, xp: work.xp, counts: work.counts.clone() });
                }
                stations.stations.push(Station { empty, working, ready, holds: def.holds, works, needs: def.needs, faster: def.faster, sound: def.sound, from_stores: def.from_stores });
            }
            for def in file.fertilisers {
                if !def.reach.is_finite() || !(0.0..=4.0).contains(&def.reach) || def.stages == 0 || !def.xp.is_finite() || def.xp < 0.0 {
                    return Err(refuse(format!("{}: a fertiliser reaches 0 to 4 blocks and grows a stage or more", def.item)));
                }
                stations.fertilisers.push(Fertiliser { item: block(&def.item)?, reach: def.reach, stages: def.stages, skill: skill(&def.skill)?, xp: def.xp });
            }
        }
        Ok(stations)
    }

    /// The station a block belongs to, and where it stands in its work.
    pub fn of(&self, block: BlockId) -> Option<(&Station, Phase)> {
        self.stations.iter().find_map(|s| match block {
            b if b == s.empty => Some((s, Phase::Empty)),
            b if b == s.working => Some((s, Phase::Working)),
            b if b == s.ready => Some((s, Phase::Ready)),
            _ => None,
        })
    }

    /// The fertiliser `item` is, if it is one.
    pub fn fertiliser(&self, item: BlockId) -> Option<&Fertiliser> {
        self.fertilisers.iter().find(|f| f.item == item)
    }

    /// Whether a right click on `block` with `held` does something: an empty station
    /// with something it works in hand, a working one (to be told how it goes), a ready
    /// one (to take).
    pub fn usable(&self, block: BlockId, held: Option<BlockId>) -> bool {
        match self.of(block) {
            Some((station, Phase::Empty)) => held.is_some_and(|item| station.work(item).is_some()),
            Some(_) => true,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player sees at a glance that a station is loaded and that it is done: its
    /// working and ready blocks each look different from the other phases on every
    /// face but the bottom.
    #[test]
    fn every_station_looks_loaded_and_done_from_every_side() {
        use crate::coords::Face;
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("content loads");
        let blocks = &content.blocks;
        assert!(!content.stations.stations.is_empty());
        for station in &content.stations.stations {
            let phases = [station.empty, station.working, station.ready];
            for face in Face::ALL.into_iter().filter(|f| *f != Face::NegY) {
                let looks: Vec<_> = phases.iter().map(|b| blocks.def(*b).textures.face(face)).collect();
                for a in 0..3 {
                    for b in a + 1..3 {
                        assert_ne!(looks[a], looks[b], "{} and {} look alike from {face:?}", blocks.def(phases[a]).name, blocks.def(phases[b]).name);
                    }
                }
            }
        }
    }

    /// The game's stations load: the rack dries meat faster by a fire and only under the
    /// open sky; the heap rots greens down four to one, faster by water; compost reaches
    /// further than a bone.
    #[test]
    fn the_games_stations_read_as_their_rules_say() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("content loads");
        let (blocks, stations) = (&content.blocks, &content.stations);
        let (rack, hung, meat) = (blocks.expect("base:drying_rack"), blocks.expect("base:drying_rack_hung"), blocks.expect("base:raw_small_game"));
        let (station, phase) = stations.of(rack).expect("the rack is a station");
        assert_eq!(phase, Phase::Empty);
        assert_eq!(stations.of(hung).map(|(_, p)| p), Some(Phase::Working));
        let (_, work) = station.work(meat).expect("the rack dries meat");
        assert_eq!(work.output, blocks.expect("base:jerky"));
        assert!(station.needs.contains(&Spot::OpenSky), "air dries it");
        assert!(station.faster.iter().any(|(s, t)| matches!(s, Spot::Near(tag, _) if tag == "fire") && *t > 1.0), "smoke dries it faster");
        assert!(station.work(blocks.expect("base:cooked_small_game")).is_none(), "cooked meat is no work for it");
        assert!(station.work(blocks.expect("base:jerky")).is_none() && station.work(blocks.expect("base:egg")).is_none(), "nor jerky, nor a raw egg");
        assert!(station.work(blocks.expect("base:raw_trout")).is_some(), "fish dries too");
        assert!(stations.usable(rack, Some(meat)) && !stations.usable(rack, None) && stations.usable(hung, None));
        let heap = blocks.expect("base:compost_heap");
        let (heap, _) = stations.of(heap).expect("the heap is a station");
        let (_, rot) = heap.work(blocks.expect("base:carrot")).expect("greens rot down");
        assert_eq!((rot.takes, rot.gives, rot.output), (4, 1, blocks.expect("base:compost")));
        let (compost, bone) = (stations.fertiliser(blocks.expect("base:compost")).unwrap(), stations.fertiliser(blocks.expect("base:bone")).unwrap());
        assert!(compost.reach > bone.reach);
    }

    /// Each station work that pays more experience for an output than making it by hand from
    /// the same input at a fire does, as words, and how many had a hand route to weigh.
    fn overpaid(content: &crate::content::Content, stations: &Stations) -> (Vec<String>, usize) {
        let (mut over, mut checked) = (Vec::new(), 0);
        for station in &stations.stations {
            for work in &station.works {
                for (input, recipe) in content.cooking.iter().filter(|(input, r)| r.output == work.output && work.accepts.contains(input)) {
                    checked += 1;
                    let hand = recipe.def.xp * work.takes as f32 / work.gives as f32;
                    if work.xp > hand {
                        over.push(format!("{} from {}: {} at a station, {hand} by hand", content.blocks.def(work.output).name, content.blocks.def(input).name, work.xp));
                    }
                }
            }
        }
        (over, checked)
    }

    /// A station never pays more experience for an output than making it by hand from the
    /// same input at a fire does (the plan: machine routes never pay more than hand). No
    /// shipped station has a hand route since the earth clamp went, so a station that would
    /// fire clay is weighed too: at the pit kiln's pay it passes, over it it is caught.
    #[test]
    fn a_station_pays_no_more_than_the_hand_route() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("content loads");
        let (over, _) = overpaid(&content, &content.stations);
        assert!(over.is_empty(), "{over:?}");
        let kiln = content.cooking.iter().find(|(input, r)| *input == content.blocks.expect("base:clay") && r.output == content.blocks.expect("base:fired_clay")).map(|(_, r)| r.def.xp).expect("clay fires by hand");
        let firing = |xp: f32| {
            let source = format!(r#"(stations: [(empty: "compost_heap", working: "compost_heap_full", ready: "compost_heap_ready", holds: 16, works: [(input: "clay", output: "fired_clay", minutes: 1.0, skill: "core:crafting", xp: {xp:?})])])"#);
            let stations = Stations::new(vec![Stations::read("base", &source).unwrap()], &content.blocks, &content.skills).unwrap();
            overpaid(&content, &stations)
        };
        assert_eq!(firing(kiln), (Vec::new(), 1), "the hand's pay, weighed");
        let (over, checked) = firing(kiln + 1.0);
        assert_eq!((over.len(), checked), (1, 1), "more than the hand's pay is caught: {over:?}");
    }

    /// The pot tells grain from vegetables by a `!` tag: wheat simmers to porridge, corn
    /// (grain and vegetable) goes in a stew with the carrots and the meat.
    #[test]
    fn the_pot_tells_grain_from_vegetables() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("content loads");
        let blocks = &content.blocks;
        let pot = content.stations.stations.iter().find(|s| blocks.def(s.empty).name == "base:cooking_pot").expect("a pot");
        let makes = |item: &str| pot.work(blocks.expect(item)).map(|(_, w)| blocks.def(w.output).name.clone());
        assert_eq!(makes("base:wheat").as_deref(), Some("base:porridge"));
        assert_eq!(makes("base:wild_corn").as_deref(), Some("base:stew"));
        assert_eq!(makes("base:carrot").as_deref(), Some("base:stew"));
        assert_eq!(makes("base:raw_beef").as_deref(), Some("base:stew"));
        assert_eq!(makes("base:cabbage").as_deref(), Some("base:stew"));
        // Nothing cooked goes in again, nor seed for planting.
        assert_eq!(makes("base:cooked_beef"), None);
        assert_eq!(makes("base:baked_potato"), None);
        assert_eq!(makes("base:wheat_seeds"), None);
    }

    #[test]
    fn a_station_that_makes_no_sense_is_refused() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("content loads");
        let refused = |source: &str| {
            let file = Stations::read("base", source).unwrap();
            Stations::new(vec![file], &content.blocks, &content.skills).err().map(|e| e.0)
        };
        let rack = |works: &str, rest: &str| format!(r#"(stations: [(empty: "drying_rack", working: "drying_rack_hung", ready: "drying_rack_dry", holds: 8, works: [{works}]{rest})])"#);
        let meat = r#"(input: "tag:raw+meat", output: "jerky", minutes: 10.0, skill: "core:cooking")"#;
        assert!(refused(&rack(meat, "")).is_none());
        assert!(refused(&rack(r#"(input: "tag:meet", output: "jerky", minutes: 10.0, skill: "core:cooking")"#, "")).is_some_and(|e| e.contains("no item has the tag")));
        assert!(refused(&rack(r#"(input: "tag:meat", output: "jerky", minutes: 0.0, skill: "core:cooking")"#, "")).is_some());
        assert!(refused(&rack(r#"(input: "tag:meat", takes: 9, output: "jerky", minutes: 10.0, skill: "core:cooking")"#, "")).is_some(), "more than a load holds");
        assert!(refused(&rack(r#"(input: "jerky", output: "jerky", minutes: 10.0, skill: "core:cooking")"#, "")).is_some_and(|e| e.contains("into itself")));
        assert!(refused(&rack(meat, r#", faster: [(Near("fire", 3), 0.5)]"#)).is_some(), "faster must be faster");
        assert!(refused(&rack(meat, r#", needs: [Near("fyre", 3)]"#)).is_some());
        assert!(refused(r#"(fertilisers: [(item: "bone", reach: 9.0)])"#).is_some());
        assert!(refused(&rack(r#"(input: "tag:raw+meat", output: "jerky", minutes: 10.0, skill: "core:cooking", counts: Some("cooked"))"#, "")).is_none());
        assert!(refused(&rack(r#"(input: "tag:raw+meat", output: "jerky", minutes: 10.0, skill: "core:cooking", counts: Some("baked"))"#, "")).is_some_and(|e| e.contains("counts as")));
    }
}
