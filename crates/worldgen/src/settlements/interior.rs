//! What stands in homes (`interiors` in settlements.ron): furniture, cloth and crockery
//! by the people who live there, so a house tells who it belongs to.
//!
//! A home is furnished once it is drawn and its residents are known: a bed for each of
//! them (upstairs first, against a wall, the head to it, back from the door), a table with
//! a seat facing it for each, cupboards and shelves, a corner of their trade (`who` names
//! a species or a work: sacks and baskets where a farmer lives, a loom for a weaver),
//! cloth on the walls and the floor in one dye, crockery in one glaze and things set on
//! tables and cupboards. Richer homes hold more and finer things, as every piece and count
//! leans on the levers and culture axes like the other lists.
//!
//! Every piece keeps the home walkable: a solid piece goes in only where every free floor
//! of its storey can still be walked to from the door or the stair, and never in front of
//! the door. Pieces are read from the canvas as drawn: a floor under, two blocks of air
//! over, so stairs, hearths and fittings already there are left alone.

use std::collections::{BTreeMap, VecDeque};

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::{IVec2, IVec3};

use super::canvas::{Canvas, Frame, Put};
use super::def::{Lean, Levers};
use super::house::{Kind, Spec};
use crate::content::{ContentError, qualify};
use crate::rng::Rng;

/// What a piece is for, which decides where it stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Role {
    /// One for each resident, against a wall, head to it.
    Bed,
    /// One to a home, where seats fit round it.
    Table,
    /// Round the table, facing it.
    Seat,
    /// Cupboards, shelves, barrels and jars against the walls.
    Store,
    /// A stove or a pot where the home has no hearth.
    Cook,
    /// On the floor, in the open.
    Rug,
    /// A lantern or a candle.
    Light,
    /// Hung on a wall at head height: banners, curtains.
    Wall,
    /// Set on a table or a cupboard: plates, bowls, jugs, baskets, candles.
    Top,
    /// The residents' trade, for those it names in `who`.
    Trade,
}

// ---------------------------------------------------------------------------------------
// As written

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InteriorsFile {
    wood: Option<String>,
    #[serde(default)]
    woods: BTreeMap<String, String>,
    #[serde(default)]
    dyes: Vec<ChoiceFile>,
    #[serde(default)]
    glazes: Vec<ChoiceFile>,
    counts: Option<CountsDef>,
    #[serde(default)]
    pieces: Vec<PieceFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChoiceFile {
    name: String,
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PieceFile {
    role: Role,
    block: String,
    #[serde(default = "one")]
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    #[serde(default)]
    who: Vec<String>,
}

fn one() -> f32 {
    1.0
}

/// How many of a role a home holds: a count from `range`, nearer the top the more its
/// `lean` favours the place.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Count {
    pub range: (u32, u32),
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    /// `lean`, parsed once at load.
    #[serde(skip)]
    parsed: Lean,
}

/// A chance, leaning like a list entry's weight (and no more than certain).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chance {
    pub chance: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    /// `lean`, parsed once at load.
    #[serde(skip)]
    parsed: Lean,
}

/// How full a home is (`counts` in `interiors`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountsDef {
    /// Beds at most, one for each resident up to it.
    pub beds: u32,
    /// Seats round the table: one for each resident and `guests` more, at most `seats`.
    pub guests: u32,
    pub seats: u32,
    pub stores: Count,
    pub trade: Count,
    pub walls: Count,
    pub lights: Count,
    /// A rug on the floor; a stove or pot where there is no hearth; something set on each
    /// table and cupboard.
    pub rug: Chance,
    pub cook: Chance,
    pub tops: Chance,
    /// A roomier home holds more: past the first `roomy` columns of floor, a store more for
    /// every two `roomy` columns, and a light and a wall cloth for every three.
    pub roomy: u32,
}

impl Default for CountsDef {
    fn default() -> Self {
        let count = |lo, hi| Count { range: (lo, hi), lean: BTreeMap::new(), parsed: Lean::default() };
        let chance = |c| Chance { chance: c, lean: BTreeMap::new(), parsed: Lean::default() };
        CountsDef { beds: 4, guests: 1, seats: 4, stores: count(1, 2), trade: count(0, 1), walls: count(0, 1), lights: count(0, 1), rug: chance(0.4), cook: chance(0.4), tops: chance(0.5), roomy: 12 }
    }
}

// ---------------------------------------------------------------------------------------
// Parsed

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub name: String,
    pub weight: f32,
    pub lean: Lean,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PieceDef {
    pub role: Role,
    /// Its block, `{wood}` the furniture wood, `{dye}` the home's dye and `{glaze}` its glaze.
    pub block: String,
    pub weight: f32,
    pub lean: Lean,
    /// Species (`farmer`, `base:farmer`) or works (`field`, `boathouse`) of residents it
    /// is for; a Trade piece needs one.
    pub who: Vec<String>,
}

/// Every package's `interiors` together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InteriorsDef {
    /// The furniture wood (`{wood}`) where the place's wood is not in `woods`.
    pub wood: String,
    /// The furniture wood for each wood a place builds with.
    pub woods: BTreeMap<String, String>,
    pub dyes: Vec<Choice>,
    pub glazes: Vec<Choice>,
    pub counts: CountsDef,
    pub pieces: Vec<PieceDef>,
}

impl InteriorsDef {
    /// Adds a package's `interiors`: its wood, woods, dyes, glazes and counts replace
    /// earlier ones where given, and its pieces add to theirs.
    pub(super) fn extend(&mut self, at: &str, package: &str, file: InteriorsFile) -> Result<(), ContentError> {
        if let Some(wood) = file.wood {
            self.wood = wood;
        }
        self.woods.extend(file.woods);
        let choices = |list: Vec<ChoiceFile>| -> Result<Vec<Choice>, ContentError> {
            list.into_iter().map(|c| Ok(Choice { lean: Lean::parse(at, &c.lean)?, name: c.name, weight: c.weight })).collect()
        };
        if !file.dyes.is_empty() {
            self.dyes = choices(file.dyes)?;
        }
        if !file.glazes.is_empty() {
            self.glazes = choices(file.glazes)?;
        }
        if let Some(mut counts) = file.counts {
            for (what, c) in [("stores", &mut counts.stores), ("trade", &mut counts.trade), ("walls", &mut counts.walls), ("lights", &mut counts.lights)] {
                c.parsed = Lean::parse(&format!("{at}: interiors counts' {what}"), &c.lean)?;
            }
            for (what, c) in [("rug", &mut counts.rug), ("cook", &mut counts.cook), ("tops", &mut counts.tops)] {
                c.parsed = Lean::parse(&format!("{at}: interiors counts' {what}"), &c.lean)?;
            }
            for (what, c) in [("stores", &counts.stores), ("trade", &counts.trade), ("walls", &counts.walls), ("lights", &counts.lights)] {
                if c.range.0 > c.range.1 || c.range.1 > 16 {
                    return Err(ContentError(format!("{at}: interiors counts' {what} must run fewest to most, at most 16")));
                }
            }
            for (what, c) in [("rug", &counts.rug), ("cook", &counts.cook), ("tops", &counts.tops)] {
                if !(0.0..=1.0).contains(&c.chance) {
                    return Err(ContentError(format!("{at}: interiors counts' {what} chance must be in [0, 1]")));
                }
            }
            if counts.roomy == 0 {
                return Err(ContentError(format!("{at}: interiors counts' roomy must be at least 1")));
            }
            if counts.beds > 16 || counts.seats > 16 {
                return Err(ContentError(format!("{at}: interiors counts' beds and seats are at most 16")));
            }
            self.counts = counts;
        }
        for p in file.pieces {
            if !p.weight.is_finite() || p.weight < 0.0 {
                return Err(ContentError(format!("{at}: interiors piece `{}` needs a weight of nothing or more", p.block)));
            }
            if p.role == Role::Trade && p.who.is_empty() {
                return Err(ContentError(format!("{at}: interiors piece `{}` is for a trade: name `who` it is for", p.block)));
            }
            self.pieces.push(PieceDef { lean: Lean::parse(at, &p.lean)?, role: p.role, block: qualify(package, &p.block), weight: p.weight, who: p.who });
        }
        Ok(())
    }

    /// The furniture woods `{wood}` may be.
    fn wood_names(&self) -> Vec<&str> {
        let mut woods: Vec<&str> = std::iter::once(self.wood.as_str()).chain(self.woods.values().map(String::as_str)).filter(|w| !w.is_empty()).collect();
        woods.sort_unstable();
        woods.dedup();
        woods
    }

    /// Every block name `piece` may be, its `{wood}`, `{dye}` and `{glaze}` filled in each way.
    fn names(&self, piece: &PieceDef) -> Vec<String> {
        let mut names = vec![piece.block.clone()];
        let woods = self.wood_names();
        let dyes: Vec<&str> = self.dyes.iter().map(|c| c.name.as_str()).collect();
        let glazes: Vec<&str> = self.glazes.iter().map(|c| c.name.as_str()).collect();
        for (key, values) in [("{wood}", &woods), ("{dye}", &dyes), ("{glaze}", &glazes)] {
            if piece.block.contains(key) {
                names = names.iter().flat_map(|n| values.iter().map(move |v| n.replace(key, v))).collect();
            }
        }
        names
    }

    /// Checks every piece names blocks in every wood, dye and glaze it may take, and that a
    /// village gives none of them away: each drops nothing or is a `furnishing`, which
    /// gives itself back only where a player placed it. Beds are beds, and what is set on
    /// tables, laid on floors, hung on walls or sat on is no solid block.
    pub(super) fn check(&self, registry: &BlockRegistry) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("settlements.ron: interiors: {what}")));
        if self.pieces.is_empty() {
            return Ok(());
        }
        if self.wood.is_empty() {
            return fail("name the furniture `wood` used where a place's wood is not in `woods`".into());
        }
        for role in [Role::Bed, Role::Table] {
            if !self.pieces.iter().any(|p| p.role == role && p.weight > 0.0) {
                return fail(format!("a home needs a {role:?}: give one piece that role"));
            }
        }
        for (what, list) in [("dye", &self.dyes), ("glaze", &self.glazes)] {
            if self.pieces.iter().any(|p| p.block.contains(&format!("{{{what}}}"))) && !list.iter().any(|c| c.weight > 0.0) {
                return fail(format!("a piece takes a `{{{what}}}` but no {what} has a weight"));
            }
        }
        for piece in &self.pieces {
            for name in self.names(piece) {
                let Some(id) = registry.id(&name) else {
                    return fail(format!("the {:?} piece `{}` is `{name}` in one of its woods, dyes or glazes, which is no block", piece.role, piece.block));
                };
                let (def, info) = (registry.def(id), registry.info(id));
                if !(info.drops.is_air() || info.drop_count == 0 || def.has_tag("furnishing")) {
                    return fail(format!("`{name}` drops itself from every home: tag it `furnishing`, or have it drop nothing"));
                }
                // A village's bed is its resident's (sleeping in one is refused by the tag).
                if piece.role == Role::Bed && !(def.has_tag("bed") && def.has_tag("furnishing")) {
                    return fail(format!("the bed `{name}` is not tagged `bed` and `furnishing`"));
                }
                if piece.role == Role::Seat && !def.has_tag("seat") {
                    return fail(format!("the seat `{name}` is not tagged `seat`"));
                }
                if matches!(piece.role, Role::Seat | Role::Rug | Role::Wall | Role::Top) && info.solid {
                    return fail(format!("the {:?} piece `{name}` is solid: it would block the way or stand on nothing", piece.role));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Resolved

/// A piece's block in each facing (north, east, south, west), and whether it is solid.
#[derive(Clone, Copy, Debug)]
struct Turned {
    ids: [BlockId; 4],
    solid: bool,
}

/// The interiors with every block name resolved, made once for a world.
#[derive(Clone, Debug)]
pub struct Interiors {
    def: InteriorsDef,
    blocks: FxHashMap<String, Turned>,
}

impl Interiors {
    /// None when no piece names a block (no interiors given, or content without them).
    pub fn new(def: &InteriorsDef, registry: &BlockRegistry) -> Option<Self> {
        let mut blocks = FxHashMap::default();
        for piece in &def.pieces {
            for name in def.names(piece) {
                let Some(id) = registry.id(&name) else { continue };
                let turn = |suffix: &str| registry.id(&format!("{name}{suffix}")).unwrap_or(id);
                let ids = [id, turn("_east"), turn("_south"), turn("_west")];
                blocks.insert(name, Turned { ids, solid: registry.info(id).solid });
            }
        }
        let beds = def.pieces.iter().any(|p| p.role == Role::Bed && def.names(p).iter().any(|n| blocks.contains_key(n)));
        beds.then(|| Interiors { def: def.clone(), blocks })
    }
}

// ---------------------------------------------------------------------------------------
// Furnishing

/// Who lives in a home: each resident's species (`base:farmer`) and the work done where
/// they work (`field`, `bakery`), if anywhere.
pub struct Household {
    pub people: Vec<(String, Option<String>)>,
}

impl InteriorsDef {
    /// Every trade piece is for someone who may live in a home: a species settlements' people
    /// are (`farmer`, `base:farmer`), or a building's work or kind (`field`, `inn`).
    pub fn check_who(&self, species: &[&str], works: &[String]) -> Result<(), ContentError> {
        let bare = |s: &str| s.rsplit(':').next().unwrap_or(s).to_owned();
        for piece in &self.pieces {
            for who in &piece.who {
                let who = bare(who);
                if !species.iter().any(|s| bare(s) == who) && !works.iter().any(|w| *w == who) {
                    return Err(ContentError(format!("settlements.ron: interiors piece `{}` is for `{who}`, who is no people's species, nor any building's work or kind", piece.block)));
                }
            }
        }
        Ok(())
    }
}

impl Household {
    fn has(&self, who: &str) -> bool {
        let bare = |s: &str| s.rsplit(':').next().unwrap_or(s).to_owned();
        let who = bare(who);
        self.people.iter().any(|(species, work)| bare(species) == who || work.as_deref() == Some(who.as_str()))
    }
}

/// Where a home's beds and seats stand, in the world: for its people to sleep and sit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Furnished {
    pub beds: Vec<IVec3>,
    pub seats: Vec<IVec3>,
    /// Things set on the table and the cupboards.
    pub tops: Vec<IVec3>,
}

const SIDES: [IVec2; 4] = [IVec2::new(0, -1), IVec2::new(1, 0), IVec2::new(0, 1), IVec2::new(-1, 0)];

/// The facing (0 north, 1 east, 2 south, 3 west) of a step `d` in the world.
fn facing_of(d: IVec2) -> usize {
    SIDES.iter().position(|&s| s == d.signum()).unwrap_or(0)
}

fn count(c: &Count, levers: &Levers, rng: &mut Rng) -> u32 {
    let bias = c.parsed.weigh(1.0, levers).max(0.05);
    let u = rng.unit().powf(1.0 / bias);
    (c.range.0 + (u * (c.range.1 - c.range.0 + 1) as f32) as u32).min(c.range.1)
}

fn chance(c: &Chance, levers: &Levers, rng: &mut Rng) -> bool {
    rng.chance(c.parsed.weigh(c.chance, levers).min(1.0))
}

fn pick<'a>(choices: &'a [Choice], levers: &Levers, rng: &mut Rng) -> Option<&'a str> {
    let weights: Vec<f32> = choices.iter().map(|c| c.lean.weigh(c.weight, levers).max(0.0)).collect();
    let total: f32 = weights.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut t = rng.unit() * total;
    for (c, w) in choices.iter().zip(&weights) {
        t -= w;
        if t <= 0.0 && *w > 0.0 {
            return Some(&c.name);
        }
    }
    choices.iter().zip(&weights).rev().find(|(_, w)| **w > 0.0).map(|(c, _)| c.name.as_str())
}

/// One storey of a home as furnishing sees it.
struct Storey {
    /// The height of its floor.
    floor: i32,
    /// Its room's columns.
    room: FxHashSet<IVec2>,
    /// Columns with a floor and two blocks of air over it: where people walk.
    walk: FxHashSet<IVec2>,
    /// Where people come in, walked from: inside the door, or beside the stairwell.
    sources: Vec<IVec2>,
    /// Where people come in or go up, which must stay reached and clear: the door, the
    /// foot of the stair, beside the stairwell.
    entries: Vec<IVec2>,
    /// The walkable columns reached from `sources` now.
    reached: FxHashSet<IVec2>,
    /// Columns nothing may stand on: in front of the door.
    kept: FxHashSet<IVec2>,
    /// Taken at the floor, and at head height.
    taken: FxHashSet<IVec2>,
    taken_high: FxHashSet<IVec2>,
    /// Solid pieces, each of which must keep a walkable column beside it.
    reached_from: Vec<IVec2>,
}

impl Storey {
    /// The walkable columns reached on foot from the ways in, with a solid piece at `without`.
    fn reach(&self, without: Option<IVec2>) -> FxHashSet<IVec2> {
        let mut seen: FxHashSet<IVec2> = self.sources.iter().copied().filter(|&e| Some(e) != without && self.walk.contains(&e)).collect();
        let mut queue: VecDeque<IVec2> = seen.iter().copied().collect();
        while let Some(p) = queue.pop_front() {
            for s in SIDES {
                let n = p + s;
                if Some(n) != without && self.walk.contains(&n) && seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        seen
    }

    /// Whether a solid piece at `p` cuts off nothing reached now: every walkable column
    /// reached stays reached, the ways in too, and every solid piece (that one too) keeps a
    /// reached column beside it.
    fn connected_without(&self, p: IVec2) -> bool {
        let seen = self.reach(Some(p));
        let beside = |q: IVec2| SIDES.iter().any(|&s| seen.contains(&(q + s)));
        seen.len() + 1 == self.reached.len() && self.entries.iter().all(|e| seen.contains(e)) && beside(p) && self.reached_from.iter().all(|&q| beside(q))
    }

    /// Steps from the nearest entry to each walkable column.
    fn distances(&self) -> FxHashMap<IVec2, u32> {
        let mut out = FxHashMap::default();
        let mut queue = VecDeque::new();
        for &e in &self.sources {
            if self.walk.contains(&e) && out.insert(e, 0).is_none() {
                queue.push_back(e);
            }
        }
        while let Some(p) = queue.pop_front() {
            let d = out[&p];
            for s in SIDES {
                let n = p + s;
                if self.walk.contains(&n) && !out.contains_key(&n) {
                    out.insert(n, d + 1);
                    queue.push_back(n);
                }
            }
        }
        out
    }

    fn free(&self, p: IVec2) -> bool {
        self.reached.contains(&p) && !self.kept.contains(&p) && !self.taken.contains(&p)
    }
}

/// Whether the canvas holds a block (not air) at `p`.
fn built(canvas: &Canvas, p: IVec3) -> bool {
    matches!(canvas.absolute.get(&p), Some(Put::Set(b)) if *b != BlockId::AIR)
}

fn air(canvas: &Canvas, p: IVec3) -> bool {
    matches!(canvas.absolute.get(&p), Some(Put::Clear))
}

struct Furnisher<'a> {
    it: &'a Interiors,
    levers: &'a Levers,
    home: &'a Household,
    rng: Rng,
    wood: String,
    dye: Option<String>,
    glaze: Option<String>,
}

impl Furnisher<'_> {
    /// A piece of `role` for this home, its block in each facing: by weight and lean, Trade
    /// pieces only for whom they name.
    fn piece(&mut self, role: Role) -> Option<Turned> {
        let def = &self.it.def;
        let fits: Vec<(Turned, f32)> = def
            .pieces
            .iter()
            .filter(|p| p.role == role && (p.who.is_empty() || p.who.iter().any(|w| self.home.has(w))))
            .filter_map(|p| {
                let mut name = p.block.replace("{wood}", &self.wood);
                if name.contains("{dye}") {
                    name = name.replace("{dye}", self.dye.as_deref()?);
                }
                if name.contains("{glaze}") {
                    name = name.replace("{glaze}", self.glaze.as_deref()?);
                }
                Some((*self.it.blocks.get(&name)?, p.lean.weigh(p.weight, self.levers)))
            })
            .filter(|(_, w)| *w > 0.0)
            .collect();
        super::plan::pick(&mut self.rng, &fits.iter().enumerate().map(|(i, (_, w))| (i, *w)).collect::<Vec<_>>()).map(|i| fits[i].0)
    }
}

/// Furnishes the home drawn from `spec` at `frame` for the people of `home`, onto the
/// canvas it was drawn on. `wood` is the place's wood; `hearth` whether it has one.
pub fn furnish(it: &Interiors, spec: &Spec, frame: &Frame, wood: &str, levers: &Levers, home: &Household, hearth: bool, canvas: &mut Canvas) -> Furnished {
    let mut out = Furnished::default();
    if spec.kind != Kind::House || !spec.inside.is_empty() {
        return out;
    }
    let def = &it.def;
    let mut rng = Rng::new(spec.seed, 0xf0_a1_57, 0, 0);
    let dye = pick(&def.dyes, levers, &mut rng).map(str::to_owned);
    let glaze = pick(&def.glazes, levers, &mut rng).map(str::to_owned);
    let wood = def.woods.get(wood).unwrap_or(&def.wood).clone();
    let mut f = Furnisher { it, levers, home, rng, wood, dye, glaze };

    // Each storey's room, walkable columns and ways in.
    let door = spec.door.clamp(1, spec.w - 2);
    let world = |x: i32, z: i32| {
        let p = frame.at(x, 0, z);
        IVec2::new(p.x, p.z)
    };
    let flights = super::house::flights(spec);
    let mut storeys: Vec<Storey> = Vec::new();
    for s in 0..spec.storeys {
        let floor = frame.origin.y + spec.stilts + s * (spec.wall + 1);
        let room: FxHashSet<IVec2> = if s == 0 { spec.room(frame).into_iter().collect() } else { spec.upper_room(frame).into_iter().collect() };
        let at = |p: IVec2, y: i32| IVec3::new(p.x, y, p.y);
        let walk: FxHashSet<IVec2> = room.iter().copied().filter(|&p| built(canvas, at(p, floor)) && air(canvas, at(p, floor + 1)) && air(canvas, at(p, floor + 2))).collect();
        let (mut entries, mut kept) = if s == 0 {
            let kept: FxHashSet<IVec2> = (door - 1..=door + 1).flat_map(|x| (0..=1).map(move |z| (x, z))).map(|(x, z)| world(x, z)).collect();
            (vec![world(door, 1)], kept)
        } else {
            // The stairwell: room columns with no floor; its ways in, the walkable columns
            // beside it.
            let hole: Vec<IVec2> = room.iter().copied().filter(|&p| air(canvas, at(p, floor))).collect();
            let entries: Vec<IVec2> = walk.iter().copied().filter(|&p| SIDES.iter().any(|&s| hole.contains(&(p + s)))).collect();
            let kept = entries.iter().copied().collect();
            (entries, kept)
        };
        // The foot of the stair up from here, the open floor beside its lowest step, is a
        // way in too, kept clear (the first of a flight is under the landing above).
        if let Some(Some(flight)) = flights.get(s as usize)
            && let Some(&(x, z)) = flight.last()
        {
            let steps: Vec<IVec2> = flight.iter().map(|&(x, z)| world(x, z)).collect();
            let low = world(x, z);
            for side in SIDES {
                let p = low + side;
                if walk.contains(&p) && !steps.contains(&p) {
                    entries.push(p);
                    kept.insert(p);
                }
            }
        }
        entries.sort_unstable_by_key(|p| (p.x, p.y));
        entries.dedup();
        let sources = if s == 0 { vec![world(door, 1)] } else { entries.clone() };
        storeys.push(Storey { floor, room, walk, sources, entries, reached: FxHashSet::default(), kept, taken: FxHashSet::default(), taken_high: FxHashSet::default(), reached_from: Vec::new() });
    }
    for st in &mut storeys {
        st.reached = st.reach(None);
    }
    // Storeys walked to: the ground from its door, and those above with a stair up.
    let order: Vec<usize> = (0..storeys.len()).filter(|&s| storeys[s].entries.iter().all(|e| storeys[s].reached.contains(e)) && !storeys[s].reached.is_empty()).collect();
    if order.is_empty() {
        return out;
    }

    // The walls beside a column: the steps from each wall into it.
    let walls_of = |st: &Storey, canvas: &Canvas, p: IVec2| -> Vec<IVec2> {
        SIDES.iter().copied().filter(|&s| !st.room.contains(&(p + s)) && built(canvas, IVec3::new(p.x + s.x, st.floor + 1, p.y + s.y))).map(|s| -s).collect()
    };
    // Places a piece at floor height (or `high`, at head height) facing `dir`, if it keeps
    // the storey walkable.
    let place = |st: &mut Storey, canvas: &mut Canvas, p: IVec2, piece: Turned, dir: IVec2, high: bool| -> bool {
        if high {
            // Over open floor (cloth on a wall), or over a piece it stands on (a top).
            if st.taken_high.contains(&p) || !(st.walk.contains(&p) || st.taken.contains(&p)) {
                return false;
            }
            canvas.set(IVec3::new(p.x, st.floor + 2, p.y), piece.ids[facing_of(dir)]);
            st.taken_high.insert(p);
            return true;
        }
        if !st.free(p) || (piece.solid && !st.connected_without(p)) {
            return false;
        }
        canvas.set(IVec3::new(p.x, st.floor + 1, p.y), piece.ids[facing_of(dir)]);
        st.taken.insert(p);
        if piece.solid {
            st.walk.remove(&p);
            st.reached_from.push(p);
            st.reached = st.reach(None);
        }
        true
    };
    // Free columns against a wall, back from the door first.
    let by_wall = |st: &Storey, canvas: &Canvas, rng: &mut Rng| -> Vec<(IVec2, IVec2)> {
        let far = st.distances();
        let mut spots: Vec<(IVec2, IVec2, u32, u32, u64)> = st
            .walk
            .iter()
            .copied()
            .filter(|&p| st.free(p))
            .filter_map(|p| {
                let walls = walls_of(st, canvas, p);
                let dir = *walls.first()?;
                Some((p, dir, far.get(&p).copied().unwrap_or(0), walls.len() as u32, 0))
            })
            .collect();
        spots.sort_unstable_by_key(|s| (s.0.x, s.0.y));
        for s in &mut spots {
            s.4 = rng.next_u64();
        }
        spots.sort_unstable_by_key(|s| (std::cmp::Reverse(s.2), std::cmp::Reverse(s.3), s.4));
        spots.into_iter().map(|s| (s.0, s.1)).collect()
    };

    // Beds: one for each resident, upstairs first.
    let people = home.people.len().max(1) as u32;
    let mut beds_left = people.min(def.counts.beds);
    let bed_order: Vec<usize> = order.iter().rev().copied().collect();
    for &s in &bed_order {
        let spots = by_wall(&storeys[s], canvas, &mut f.rng);
        for (p, dir) in spots {
            if beds_left == 0 {
                break;
            }
            let Some(bed) = f.piece(Role::Bed) else { break };
            if place(&mut storeys[s], canvas, p, bed, dir, false) {
                out.beds.push(IVec3::new(p.x, storeys[s].floor + 1, p.y));
                beds_left -= 1;
            }
        }
    }

    // The table, on the ground floor where seats fit round it, and its seats.
    let ground = order[0];
    let mut surfaces: Vec<(usize, IVec2)> = Vec::new();
    let table = f.piece(Role::Table);
    for &at in &order {
        if !surfaces.is_empty() {
            break;
        }
        let st = &storeys[at];
        let centre = st.walk.iter().fold(IVec2::ZERO, |a, &p| a + p).as_vec2() / st.walk.len().max(1) as f32;
        let mut spots: Vec<(IVec2, usize, f32)> = st.walk.iter().copied().filter(|&p| st.free(p)).map(|p| (p, SIDES.iter().filter(|&&s| st.free(p + s)).count(), p.as_vec2().distance(centre))).collect();
        spots.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.2.total_cmp(&b.2)).then((a.0.x, a.0.y).cmp(&(b.0.x, b.0.y))));
        if let Some(table) = table {
            for (p, ..) in spots {
                if place(&mut storeys[at], canvas, p, table, IVec2::new(0, -1), false) {
                    surfaces.push((at, p));
                    let seats = (people + def.counts.guests).min(def.counts.seats);
                    let mut sat = 0;
                    for s in SIDES {
                        if sat >= seats {
                            break;
                        }
                        let Some(seat) = f.piece(Role::Seat) else { break };
                        if place(&mut storeys[at], canvas, p + s, seat, -s, false) {
                            out.seats.push(IVec3::new(p.x + s.x, storeys[at].floor + 1, p.y + s.y));
                            sat += 1;
                        }
                    }
                    break;
                }
            }
        }
    }

    // Against the walls: a stove where there is no hearth, the residents' trade, stores.
    let floor: usize = order.iter().map(|&s| storeys[s].reached.len()).sum();
    let roomy = (floor as u32 / def.counts.roomy).saturating_sub(1);
    let mut wall_pieces: Vec<Role> = Vec::new();
    if !hearth && chance(&def.counts.cook, levers, &mut f.rng) {
        wall_pieces.push(Role::Cook);
    }
    for _ in 0..count(&def.counts.trade, levers, &mut f.rng) {
        wall_pieces.push(Role::Trade);
    }
    for _ in 0..count(&def.counts.stores, levers, &mut f.rng) + roomy / 2 {
        wall_pieces.push(Role::Store);
    }
    for _ in 0..count(&def.counts.lights, levers, &mut f.rng) + roomy / 3 {
        wall_pieces.push(Role::Light);
    }
    for role in wall_pieces {
        let Some(piece) = f.piece(role) else { continue };
        'storeys: for &s in &order {
            for (p, dir) in by_wall(&storeys[s], canvas, &mut f.rng) {
                if place(&mut storeys[s], canvas, p, piece, dir, false) {
                    if piece.solid && matches!(role, Role::Store | Role::Cook) {
                        surfaces.push((s, p));
                    }
                    break 'storeys;
                }
            }
        }
    }

    // Cloth: on the walls at head height, and a rug on the floor in the open.
    for _ in 0..count(&def.counts.walls, levers, &mut f.rng) + roomy / 3 {
        let Some(piece) = f.piece(Role::Wall) else { break };
        let s = order[f.rng.below(order.len() as u32) as usize];
        let spots = by_wall_high(&storeys[s], canvas, &walls_of);
        let Some(&(p, dir)) = spots.get(f.rng.below(spots.len().max(1) as u32) as usize) else { continue };
        place(&mut storeys[s], canvas, p, piece, dir, true);
    }
    if chance(&def.counts.rug, levers, &mut f.rng)
        && let Some(rug) = f.piece(Role::Rug)
    {
        let st = &storeys[ground];
        let mut open: Vec<IVec2> = st.walk.iter().copied().filter(|&p| st.free(p) && walls_of(st, canvas, p).is_empty()).collect();
        open.sort_unstable_by_key(|p| (p.x, p.y));
        if !open.is_empty() {
            let p = open[f.rng.below(open.len() as u32) as usize];
            place(&mut storeys[ground], canvas, p, rug, IVec2::new(0, -1), false);
        }
    }

    // Things set on the table and the cupboards.
    for (s, p) in surfaces {
        if !chance(&def.counts.tops, levers, &mut f.rng) {
            continue;
        }
        let Some(top) = f.piece(Role::Top) else { continue };
        let dir = SIDES[f.rng.below(4) as usize];
        if place(&mut storeys[s], canvas, p, top, dir, true) {
            out.tops.push(IVec3::new(p.x, storeys[s].floor + 2, p.y));
        }
    }
    out
}

/// Free columns against a wall with air at head height, as (column, step away from the wall).
fn by_wall_high(st: &Storey, canvas: &Canvas, walls_of: &dyn Fn(&Storey, &Canvas, IVec2) -> Vec<IVec2>) -> Vec<(IVec2, IVec2)> {
    let mut spots: Vec<(IVec2, IVec2)> = st
        .walk
        .iter()
        .copied()
        .filter(|&p| !st.taken_high.contains(&p) && !st.kept.contains(&p))
        .filter_map(|p| walls_of(st, canvas, p).first().map(|&d| (p, d)))
        .collect();
    spots.sort_unstable_by_key(|s| (s.0.x, s.0.y));
    spots
}
