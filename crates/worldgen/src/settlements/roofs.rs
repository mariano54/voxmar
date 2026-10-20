//! Roofs laid in shaped pieces, and the fire they are kept from.
//!
//! A style's `roof_pieces` give a pitched gable its slope pieces, each turned with its low
//! edge to the eave below it, and a ridge cap along the peak. After a settlement is drawn,
//! `keep_from_flames` walks out from each of its flames as a home's fire catches (out a
//! face at a time, on only through open cells, `rules.fire.reach` steps). A street lamp or
//! fire that reaches something that catches goes; a roof of pieces that catch (thatch) that
//! a building's own flame reaches is laid in the fire's `instead` pieces, turned the same
//! way; a flame that still reaches something that catches is put out. So no village stands
//! ready to go up, whatever fire.ron's reach (at most 4).

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::{IVec2, IVec3};

use super::canvas::{Canvas, Put};
use super::def::{FireDef, RoofPiecesDef, fill};

/// A facing block's names in the world's turns: north (the bare name), east, south, west.
pub const SUFFIXES: [&str; 4] = ["", "_east", "_south", "_west"];
/// The steps each facing looks along, in the order of `SUFFIXES`.
pub const SIDES: [IVec2; 4] = [IVec2::new(0, -1), IVec2::new(1, 0), IVec2::new(0, 1), IVec2::new(-1, 0)];

/// The facing (0 north, 1 east, 2 south, 3 west) nearest the step `d`.
pub fn facing_of(d: IVec2) -> usize {
    SIDES.iter().position(|&s| s == d.signum()).unwrap_or(0)
}

/// A roof's slope and ridge, each in its four facings: a slope's low edge, and a ridge's
/// run (north for one running east to west), the way its facing says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoofPieces {
    pub slope: [BlockId; 4],
    pub ridge: [BlockId; 4],
}

impl RoofPieces {
    /// The pieces `def` names for a building of `wood` (and `trim`), when every facing of
    /// both is a block.
    pub fn resolve(def: &RoofPiecesDef, wood: &str, trim: &str, id: impl Fn(&str) -> Option<BlockId>) -> Option<Self> {
        let turned = |name: &str| -> Option<[BlockId; 4]> {
            let name = fill(name, wood, trim);
            let mut ids = [BlockId::AIR; 4];
            for (i, suffix) in SUFFIXES.iter().enumerate() {
                ids[i] = id(&format!("{name}{suffix}"))?;
            }
            Some(ids)
        };
        Some(Self { slope: turned(&def.slope)?, ridge: turned(&def.ridge)? })
    }
}

/// `rules.fire` with its tags looked up once, for one world's blocks.
#[derive(Clone, Debug)]
pub struct Fire {
    flame: Vec<bool>,
    catch: Vec<bool>,
    solid: Vec<bool>,
    pub reach: i32,
    pub instead: Option<RoofPieces>,
    /// Every roof piece settlements lay, whether a slope or a ridge, and its facing.
    pieces: FxHashMap<BlockId, (bool, usize)>,
}

impl Fire {
    pub fn new(def: &FireDef, registry: &BlockRegistry, all: impl IntoIterator<Item = RoofPieces>) -> Self {
        let ids: Vec<BlockId> = registry.ids().collect();
        let size = ids.iter().map(|b| b.0 as usize + 1).max().unwrap_or(0);
        let (mut flame, mut catch, mut solid) = (vec![false; size], vec![false; size], vec![false; size]);
        for b in ids {
            let d = registry.def(b);
            flame[b.0 as usize] = def.flames.iter().any(|t| d.has_tag(t));
            catch[b.0 as usize] = def.catch.iter().any(|t| d.has_tag(t));
            solid[b.0 as usize] = registry.info(b).solid;
        }
        let instead = def.instead.as_ref().and_then(|p| RoofPieces::resolve(p, "", "", |n| registry.id(n)));
        let mut pieces = FxHashMap::default();
        for p in all.into_iter().chain(instead) {
            for i in 0..4 {
                pieces.insert(p.slope[i], (false, i));
                pieces.insert(p.ridge[i], (true, i));
            }
        }
        Self { flame, catch, solid, reach: def.reach, instead, pieces }
    }

    fn get(list: &[bool], b: BlockId) -> bool {
        list.get(b.0 as usize).copied().unwrap_or(false)
    }

    pub fn flame(&self, b: BlockId) -> bool {
        Self::get(&self.flame, b)
    }

    pub fn catches(&self, b: BlockId) -> bool {
        Self::get(&self.catch, b)
    }

    /// Whether `b` is a roof piece settlements lay.
    pub fn piece(&self, b: BlockId) -> bool {
        self.pieces.contains_key(&b)
    }

    /// Every cell a flame at `from` reaches within the fire's reach, as a home's fire goes:
    /// out a face at a time, on only through cells that are not solid. A cell reached is
    /// visited whether solid or not. `at` gives the block in a cell (open where none).
    pub fn reached(&self, from: IVec3, at: impl Fn(IVec3) -> BlockId, mut visit: impl FnMut(IVec3, BlockId)) {
        let mut seen = FxHashSet::default();
        seen.insert(from);
        let mut ring = vec![from];
        for _ in 0..self.reach {
            let mut next = Vec::new();
            for cell in ring {
                for side in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
                    let pos = cell + side;
                    if !seen.insert(pos) {
                        continue;
                    }
                    let block = at(pos);
                    visit(pos, block);
                    if !Self::get(&self.solid, block) {
                        next.push(pos);
                    }
                }
            }
            ring = next;
        }
    }

    /// The cells of what catches that some flame in `blocks` reaches.
    pub fn exposed(&self, blocks: &FxHashMap<IVec3, BlockId>) -> Vec<(IVec3, IVec3)> {
        let at = |p: IVec3| blocks.get(&p).copied().unwrap_or(BlockId::AIR);
        let mut found = Vec::new();
        let mut flames: Vec<IVec3> = blocks.iter().filter(|(_, b)| self.flame(**b)).map(|(p, _)| *p).collect();
        flames.sort_unstable_by_key(|p| (p.y, p.x, p.z));
        for f in flames {
            self.reached(f, at, |p, b| {
                if self.catches(b) {
                    found.push((f, p));
                }
            });
        }
        found
    }

    /// Takes away every flame on the ground (a lamp, post and all, or a fire) that reaches
    /// something that catches, lays every catching roof a building's flame reaches in
    /// `instead`, turned the same way, then puts out any flame that still reaches.
    /// `ground` gives where a block set over the ground of a column stands. Returns how
    /// many pieces were laid again and flames put out.
    pub fn keep_from_flames(&self, canvas: &mut Canvas, mut ground: impl FnMut(i32, i32) -> i32) -> (usize, usize) {
        let mut swapped = 0;
        let mut out = 0;
        for round in 0..2 {
            let mut blocks: FxHashMap<IVec3, BlockId> = FxHashMap::default();
            let mut from: FxHashMap<IVec3, (bool, IVec3)> = FxHashMap::default();
            for (&p, &put) in &canvas.absolute {
                blocks.insert(p, block_of(put));
                from.insert(p, (false, p));
            }
            for (&p, &put) in &canvas.grounded {
                let at = IVec3::new(p.x, ground(p.x, p.z) + p.y, p.z);
                blocks.insert(at, block_of(put));
                from.insert(at, (true, p));
            }
            let exposed = self.exposed(&blocks);
            if exposed.is_empty() {
                break;
            }
            if round == 0 {
                // A lamp or a fire standing out on the ground (a prop) goes, post and all,
                // rather than the roof it would reach.
                let mut street = Vec::new();
                for &(f, _) in &exposed {
                    let (grounded, key) = from[&f];
                    if grounded && !street.contains(&key) {
                        street.push(key);
                    }
                }
                for key in &street {
                    for up in 1..=key.y {
                        canvas.grounded.remove(&IVec3::new(key.x, up, key.z));
                    }
                    out += 1;
                }
                let Some(instead) = &self.instead else { continue };
                // The whole roof a piece reached from a building is part of, so none is
                // laid half in each.
                let piece = |p: &IVec3| blocks.get(p).is_some_and(|&b| self.catches(b) && self.pieces.contains_key(&b));
                let mut roof: Vec<IVec3> = exposed.iter().filter(|e| !from[&e.0].0).map(|e| e.1).filter(piece).collect();
                let mut seen: FxHashSet<IVec3> = roof.iter().copied().collect();
                while let Some(p) = roof.pop() {
                    let (ridge, i) = self.pieces[&blocks[&p]];
                    let block = if ridge { instead.ridge[i] } else { instead.slope[i] };
                    let (grounded, key) = from[&p];
                    let map = if grounded { &mut canvas.grounded } else { &mut canvas.absolute };
                    map.insert(key, Put::Set(block));
                    swapped += 1;
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            for dx in -1..=1 {
                                let n = p + IVec3::new(dx, dy, dz);
                                if piece(&n) && seen.insert(n) {
                                    roof.push(n);
                                }
                            }
                        }
                    }
                }
                continue;
            }
            let mut flames: Vec<IVec3> = exposed.iter().map(|e| e.0).collect();
            flames.sort_unstable_by_key(|p| (p.y, p.x, p.z));
            flames.dedup();
            for f in flames {
                let (grounded, key) = from[&f];
                let map = if grounded { &mut canvas.grounded } else { &mut canvas.absolute };
                map.insert(key, Put::Clear);
                out += 1;
            }
            break;
        }
        (swapped, out)
    }
}

/// What a put leaves standing (air for a clearing).
fn block_of(put: Put) -> BlockId {
    match put {
        Put::Set(b) | Put::Soft(b) | Put::Down(b) => b,
        Put::Clear => BlockId::AIR,
    }
}
