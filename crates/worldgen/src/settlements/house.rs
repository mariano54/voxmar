//! Houses and the other buildings of a settlement, drawn block by block in their own
//! coordinates: the front at z = 0 looking towards −z, x across the front from 0 to
//! `w − 1`, z to the back at `d − 1`, the floor at y = 0.
//!
//! Every house is rolled: its kind (`houses` in settlements.ron) sets its size, walls,
//! storeys and form (four walls, round, an A-frame, rooms round a court, a shop under
//! rooms); its roof is one of the style's shapes (gable, hipped, half-hipped, saltbox,
//! gambrel, conical, flat or a single slope), steep or low; its extras (a chimney with a
//! hearth, a porch, a shed, a wing, dormers, a balcony, an outside stair, shutters, an
//! awning, a sign, a rain barrel, a weathervane, a dovecote, herbs drying under the eaves,
//! flower boxes, a lamp, a woodpile, moss in old walls) each come by their own chance,
//! leaning on the settlement's levers; a working building has its fittings inside (an
//! oven, a millstone, vats, racks, stalls, bins, a smoking fire, an altar and pews, a
//! boat, a counter). Its materials come from the few the settlement keeps of its style's.
//! Windows fall in a regular rhythm in orderly places and anywhere in others.

use voxmar_engine::block::BlockId;
use voxmar_engine::glam::IVec2;

use super::canvas::{Canvas, Facing, Frame, Palette, Put, unit};
use super::def::{Centre, Extra, Fitting, Form, Roof};
use crate::rng::{Rng, hash3};

/// What a building is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    House,
    Trader,
    Inn,
    Smithy,
    Stall,
    Well,
    Shrine,
    Field,
    Pen,
    Orchard,
    Woodyard,
    /// The one great thing a village is known by.
    Centrepiece,
}

impl Kind {
    pub const ALL: [Kind; 12] =
        [Kind::House, Kind::Trader, Kind::Inn, Kind::Smithy, Kind::Stall, Kind::Well, Kind::Shrine, Kind::Field, Kind::Pen, Kind::Orchard, Kind::Woodyard, Kind::Centrepiece];

    pub fn name(self) -> &'static str {
        match self {
            Kind::House => "house",
            Kind::Trader => "trader",
            Kind::Inn => "inn",
            Kind::Smithy => "smithy",
            Kind::Stall => "stall",
            Kind::Well => "well",
            Kind::Shrine => "shrine",
            Kind::Field => "field",
            Kind::Pen => "pen",
            Kind::Orchard => "orchard",
            Kind::Woodyard => "woodyard",
            Kind::Centrepiece => "centrepiece",
        }
    }

    /// Whether people live or work inside it.
    pub fn indoors(self) -> bool {
        matches!(self, Kind::House | Kind::Trader | Kind::Inn)
    }
}

/// The blocks one building is made of.
#[derive(Clone, Debug)]
pub struct Mats {
    pub foundation: Palette,
    pub floor: BlockId,
    pub wall: BlockId,
    pub frame: BlockId,
    pub roof: BlockId,
    pub trim: BlockId,
    pub window: Option<BlockId>,
    pub weathered: Palette,
    pub hedge: Palette,
    pub log: BlockId,
    /// Its chimney, one block all the way up.
    pub chimney: BlockId,
    /// The crops its fields grow, by weight: their stages, the last ripe.
    pub crops: Vec<(Vec<BlockId>, f32)>,
    /// The fruit trees it grows: only those of its size's crop level or below.
    pub fruit: Palette,
    /// The pieces a pitched gable may be laid in, by weight, and the share of such roofs
    /// that are.
    pub roof_pieces: Vec<(super::roofs::RoofPieces, f32)>,
    pub pieced: f32,
    /// The beam a jettied house's upper storeys stand out on, in each facing, and the share
    /// of houses that can be that are.
    pub jetty: Option<([BlockId; 4], f32)>,
}

impl Mats {
    /// The pieces a pitched gable roof of the building `seed` is laid in, if it is.
    pub fn pieces(&self, seed: u64) -> Option<super::roofs::RoofPieces> {
        if self.roof_pieces.is_empty() || unit(hash3(seed, 0x7007, 0, 0)) >= self.pieced {
            return None;
        }
        let total: f32 = self.roof_pieces.iter().map(|p| p.1).sum();
        let mut t = unit(hash3(seed, 0x7007, 1, 0)) * total;
        for &(p, w) in &self.roof_pieces {
            if t < w {
                return Some(p);
            }
            t -= w;
        }
        self.roof_pieces.last().map(|p| p.0)
    }

    /// The beams the building `seed` is jettied on, if it is.
    pub fn jetty(&self, seed: u64) -> Option<[BlockId; 4]> {
        self.jetty.filter(|j| unit(hash3(seed, 0x7e77, 0, 0)) < j.1).map(|j| j.0)
    }

    /// The first stone of the foundation, for hearths, ovens and plinths.
    pub fn stone(&self) -> BlockId {
        self.foundation.0.first().map_or(self.wall, |b| b.0)
    }
}

/// Blocks every settlement uses, whatever its style.
#[derive(Clone, Debug)]
pub struct Fixed {
    pub torch: BlockId,
    pub campfire: BlockId,
    pub table: BlockId,
    pub farmland: BlockId,
    pub water: BlockId,
    pub planter: Option<BlockId>,
    pub anvil: BlockId,
    pub barrel: Option<BlockId>,
    pub bell: Option<BlockId>,
    pub altar: Option<BlockId>,
    pub flowers: Palette,
    pub bushes: Palette,
    pub canopies: Palette,
    pub compost: Palette,
    pub boulder: Palette,
    /// Fruit trees' leaves, by weight, with the Farming level of their fruit.
    pub fruit: Vec<(BlockId, f32, u32)>,
    pub hay: Palette,
    pub hanging: Palette,
    pub nets: Palette,
    pub sacks: Palette,
    pub crates: Palette,
    pub cloth: Palette,
    /// What stands in homes; none where content gives no interiors.
    pub interiors: Option<super::interior::Interiors>,
    /// What village flames are kept from (`rules.fire`).
    pub fire: super::roofs::Fire,
    pub dirt: BlockId,
    /// The chance of a stone lowest course, and old walls' share of weathered blocks.
    pub plinth: f32,
    pub moss: f32,
}

/// A house rolled before it is placed, so its footprint is known.
#[derive(Clone, Debug)]
pub struct Spec {
    pub kind: Kind,
    pub w: i32,
    pub d: i32,
    pub wall: i32,
    pub storeys: i32,
    pub roof: Roof,
    /// Blocks inwards per step up: 1 is steep, 2 low.
    pub pitch: i32,
    /// Whether the ridge runs across the front (along x) rather than front to back.
    pub ridge_x: bool,
    pub extras: Vec<Extra>,
    pub door: i32,
    /// Shed on the right (+x) side: its depth.
    pub shed: i32,
    /// A wing behind, on the right: its width and depth.
    pub wing: Option<(i32, i32)>,
    pub stilts: i32,
    /// Windows in a regular rhythm.
    pub regular: bool,
    pub window_chance: f32,
    pub form: Form,
    /// What stands inside a working building.
    pub inside: Vec<Fitting>,
    /// What a centrepiece is, when the code draws it.
    pub centre: Option<Centre>,
    pub seed: u64,
    /// A house of a row (a terrace), built on against its neighbours: none of its eaves or
    /// side windows on a side another stands against.
    pub row: Option<Row>,
}

/// A house's place in a row of houses built wall to wall along a street, each with its own
/// side walls (`left` at x 0, `right` at x `w - 1`) against the next one's. The row's
/// `seed` decides what all its houses share, jettied or not and the pieces of their roofs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub seed: u64,
    pub left: bool,
    pub right: bool,
}

impl Spec {
    /// The seed for what all the houses of its row share.
    pub fn shared_seed(&self) -> u64 {
        self.row.map_or(self.seed, |r| r.seed)
    }

    pub fn has(&self, extra: Extra) -> bool {
        self.extras.contains(&extra)
    }

    /// Indoor ground-floor columns, using the same footprint as the painted floor.
    /// Furniture and headroom are checked against the live blocks by gameplay.
    pub fn room(&self, frame: &Frame) -> Vec<IVec2> {
        if !self.kind.indoors() {
            return Vec::new();
        }
        let foot = Foot { x0: 0, z0: 0, w: self.w, d: self.d, form: self.form };
        let mut local: Vec<_> = (0..self.d).flat_map(|z| (0..self.w).map(move |x| (x, z)))
            .filter(|&(x, z)| foot.room(x, z)).collect();
        if let Some((ww, wd)) = self.wing {
            // The back wall is opened into this wing, including its joining row.
            local.extend((self.d - 1..self.d + wd - 1)
                .flat_map(|z| (self.w - ww + 1..self.w - 1).map(move |x| (x, z))));
        }
        local.into_iter().map(|(x, z)| {
            let p = frame.at(x, 0, z);
            IVec2::new(p.x, p.z)
        }).collect()
    }

    /// The room columns of the storeys above the ground, which no wing reaches.
    pub fn upper_room(&self, frame: &Frame) -> Vec<IVec2> {
        let foot = Foot { x0: 0, z0: 0, w: self.w, d: self.d, form: self.form };
        (0..self.d)
            .flat_map(|z| (0..self.w).map(move |x| (x, z)))
            .filter(|&(x, z)| foot.room(x, z))
            .map(|(x, z)| {
                let p = frame.at(x, 0, z);
                IVec2::new(p.x, p.z)
            })
            .collect()
    }

    /// The building's own box, everything it puts down included: overhangs, porch,
    /// shed, wing, chimney, the lamp and step at its door.
    pub fn envelope(&self) -> (IVec2, IVec2) {
        match self.kind {
            Kind::Well | Kind::Shrine | Kind::Stall | Kind::Centrepiece => (IVec2::ZERO, IVec2::new(self.w - 1, self.d - 1)),
            Kind::Field | Kind::Pen | Kind::Orchard | Kind::Woodyard => (IVec2::ZERO, IVec2::new(self.w - 1, self.d - 1)),
            Kind::Smithy => (IVec2::new(-1, -2), IVec2::new(self.w, self.d)),
            _ => {
                let front = if self.has(Extra::Porch) || (self.kind == Kind::Trader && self.form != Form::ShopFront) {
                    -4
                } else if self.form == Form::ShopFront {
                    -3
                } else {
                    -2
                };
                let right = self.w + if self.shed > 0 { 3 } else { 0 };
                let back = self.d + self.wing.map_or(0, |(_, wd)| wd);
                // No eave over a side another house of its row stands against.
                let (left, right) = match self.row {
                    Some(row) => (if row.left { 0 } else { -1 }, if row.right { right - 1 } else { right }),
                    None => (-1, right),
                };
                (IVec2::new(left, front), IVec2::new(right, back))
            }
        }
    }

    /// Where its chimney stands, inside the walls, and the wall behind it: over the oven
    /// where it bakes; else against a gable end under the ridge, the back one where the
    /// ridge runs front to back and no wing joins there, the left one otherwise.
    pub fn chimney(&self) -> Option<((i32, i32), (i32, i32))> {
        if !self.has(Extra::Chimney) {
            return None;
        }
        let (w, d) = (self.w, self.d);
        if self.inside.contains(&Fitting::Oven) {
            let x = 2.min(w - 2);
            return Some(((x, d - 2), (x, d - 1)));
        }
        let gabled = matches!(self.roof, Roof::Gable | Roof::HalfHip | Roof::Saltbox | Roof::Gambrel);
        // The altar occupies the back centre; its hearth must use the side wall.
        let back = !self.inside.contains(&Fitting::Altar) && self.wing.is_none_or(|(ww, _)| w / 2 < w - ww);
        if gabled && !self.ridge_x && back { Some(((w / 2, d - 2), (w / 2, d - 1))) } else { Some(((1, d / 2), (0, d / 2))) }
    }

    /// The top of the walls, over the floor.
    pub fn top(&self) -> i32 {
        self.storeys * (self.wall + 1) - 1
    }
}

/// Draws a building onto the canvas.
pub fn draw(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    match spec.kind {
        Kind::House | Kind::Trader | Kind::Inn => house(spec, frame, mats, fixed, canvas),
        Kind::Smithy => smithy(spec, frame, mats, fixed, canvas),
        Kind::Stall => stall(spec, frame, mats, fixed, canvas),
        Kind::Well => well(spec, frame, mats, fixed, canvas),
        Kind::Shrine => shrine(spec, frame, mats, fixed, canvas),
        Kind::Field => field(spec, frame, mats, fixed, canvas),
        Kind::Pen => pen(spec, frame, mats, fixed, canvas),
        Kind::Orchard => orchard(spec, frame, mats, fixed, canvas),
        Kind::Woodyard => woodyard(spec, frame, mats, fixed, canvas),
        // The planner draws centrepieces, which need their parts.
        Kind::Centrepiece => {}
    }
}

/// Puts blocks in a building's own coordinates.
pub(super) struct Pen<'a> {
    pub frame: &'a Frame,
    pub canvas: &'a mut Canvas,
    pub seed: u64,
}

impl Pen<'_> {
    /// The highest block set at `x`, `z` from `low` to `high`, if any.
    fn highest(&self, x: i32, z: i32, low: i32, high: i32) -> Option<i32> {
        (low..=high).rev().find(|&y| matches!(self.canvas.absolute.get(&self.frame.at(x, y, z)), Some(Put::Set(_))))
    }

    pub fn set(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
        self.canvas.set(self.frame.at(x, y, z), block);
    }

    pub fn put(&mut self, x: i32, y: i32, z: i32, put: Put) {
        self.canvas.put(self.frame.at(x, y, z), put);
    }

    pub fn clear(&mut self, x: i32, y: i32, z: i32) {
        self.put(x, y, z, Put::Clear);
    }

    pub fn pal(&mut self, x: i32, y: i32, z: i32, palette: &Palette) {
        let pos = self.frame.at(x, y, z);
        if let Some(block) = palette.at(self.seed, pos) {
            self.canvas.set(pos, block);
        }
    }

    /// On the ground of the column under (x, z), `up` over its top.
    pub fn ground(&mut self, x: i32, up: i32, z: i32, put: Put) {
        let p = self.frame.at(x, 0, z);
        self.canvas.on_ground(p.x, p.z, up, put);
    }

    pub fn roll(&self, x: i32, y: i32, z: i32, salt: u64) -> f32 {
        let p = self.frame.at(x, y, z);
        unit(hash3(self.seed ^ salt, p.x, p.y, p.z))
    }
}

/// How far in from the edge a courtyard house's court begins.
const COURT: i32 = 4;

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Which columns of its box a building stands on.
#[derive(Clone, Copy, Debug)]
struct Foot {
    x0: i32,
    z0: i32,
    w: i32,
    d: i32,
    form: Form,
}

impl Foot {
    fn boxed(x0: i32, z0: i32, w: i32, d: i32) -> Self {
        Foot { x0, z0, w, d, form: Form::Box }
    }

    fn inside(&self, x: i32, z: i32) -> bool {
        let (x, z) = (x - self.x0, z - self.z0);
        if x < 0 || z < 0 || x >= self.w || z >= self.d {
            return false;
        }
        match self.form {
            Form::Round => {
                let c = self.w / 2;
                let (dx, dz) = (x - c, z - c);
                ((dx * dx + dz * dz) as f32) <= (c as f32 + 0.5).powi(2)
            }
            Form::Courtyard => !(x >= COURT && x < self.w - COURT && z >= COURT && z < self.d - COURT),
            _ => true,
        }
    }

    /// Whether (x, z) is in its walls: inside, beside a column outside.
    fn wall(&self, x: i32, z: i32) -> bool {
        self.inside(x, z) && SIDES.iter().any(|&(dx, dz)| !self.inside(x + dx, z + dz))
    }

    /// Whether (x, z) is where two walls meet.
    fn corner(&self, x: i32, z: i32) -> bool {
        self.wall(x, z) && (!self.inside(x - 1, z) || !self.inside(x + 1, z)) && (!self.inside(x, z - 1) || !self.inside(x, z + 1))
    }

    /// Whether (x, z) is a floor to stand on, clear of the walls.
    fn room(&self, x: i32, z: i32) -> bool {
        self.inside(x, z) && !self.wall(x, z)
    }
}

/// The height of a roof over each column it covers, its eaves one column out.
pub(super) struct Heights {
    x0: i32,
    z0: i32,
    w: i32,
    d: i32,
    h: Vec<Option<i32>>,
}

impl Heights {
    fn new(foot: &Foot) -> Self {
        Heights { x0: foot.x0, z0: foot.z0, w: foot.w, d: foot.d, h: vec![None; ((foot.w + 2) * (foot.d + 2)) as usize] }
    }

    fn index(&self, x: i32, z: i32) -> Option<usize> {
        let (i, j) = (x - self.x0 + 1, z - self.z0 + 1);
        (i >= 0 && j >= 0 && i < self.w + 2 && j < self.d + 2).then(|| (j * (self.w + 2) + i) as usize)
    }

    pub fn at(&self, x: i32, z: i32) -> Option<i32> {
        self.index(x, z).and_then(|i| self.h[i])
    }

    fn put(&mut self, x: i32, z: i32, y: i32) {
        if let Some(i) = self.index(x, z) {
            self.h[i] = Some(y);
        }
    }

    /// The highest column nearest the middle, and its height.
    pub fn peak(&self) -> Option<(i32, i32, i32)> {
        let (cx, cz) = (self.x0 * 2 + self.w - 1, self.z0 * 2 + self.d - 1);
        let mut best: Option<(i32, i32, i32, i32)> = None;
        for z in self.z0 - 1..=self.z0 + self.d {
            for x in self.x0 - 1..=self.x0 + self.w {
                if let Some(y) = self.at(x, z) {
                    let off = (2 * x - cx).abs() + (2 * z - cz).abs();
                    if best.is_none_or(|b| y > b.2 || (y == b.2 && off < b.3)) {
                        best = Some((x, z, y, off));
                    }
                }
            }
        }
        best.map(|(x, z, y, _)| (x, z, y))
    }
}

/// A roof over the walls of `foot`, whose walls top out at `top`: the roof block over
/// every column it covers, the walls carried up under it, the loft within cleared, and
/// `windows` rows of window in the middle of each gable end.
#[allow(clippy::too_many_arguments)]
fn roof(spec: &Spec, pen: &mut Pen, mats: &Mats, top: i32, foot: &Foot, ridge_x: bool, shape: Roof, block: BlockId, windows: i32, blind: Option<(i32, i32)>) -> Heights {
    let pitch = spec.pitch.max(1);
    let Foot { x0, z0, w, d, .. } = *foot;
    let flat = shape == Roof::Flat;
    // Eaves one column out, but an A-frame's timber ends stand flush with its roof.
    let a_frame = foot.form == Form::AFrame;
    // A row's house has no eave over a side its neighbour stands against: their roofs run on.
    let (open_left, open_right) = spec.row.map_or((true, true), |r| (!r.left, !r.right));
    let roofed = |x: i32, z: i32| {
        if (x < x0 && !open_left) || (x >= x0 + w && !open_right) {
            false
        } else if flat {
            foot.inside(x, z)
        } else if a_frame {
            let (along, across) = if ridge_x { (x - x0, z) } else { (z - z0, x) };
            along >= 0 && along < if ridge_x { w } else { d } && (-1..=1).any(|o| if ridge_x { foot.inside(x, across + o) } else { foot.inside(across + o, z) })
        } else {
            (-1..=1).any(|dz| (-1..=1).any(|dx| foot.inside(x + dx, z + dz)))
        }
    };
    let (span, run) = if ridge_x { (d, w) } else { (w, d) };
    // Hipped: the steps in from the nearest eave.
    let hip = |x: i32, z: i32| -> i32 {
        let mut best = i32::MAX;
        for zz in z0 - 2..=z0 + d + 1 {
            for xx in x0 - 2..=x0 + w + 1 {
                if !roofed(xx, zz) {
                    best = best.min((xx - x).abs().max((zz - z).abs()));
                }
            }
        }
        best - 1
    };
    let mut heights = Heights::new(foot);
    for z in z0 - 1..=z0 + d {
        for x in x0 - 1..=x0 + w {
            if !roofed(x, z) {
                continue;
            }
            let (s, r) = if ridge_x { (z - z0, x - x0) } else { (x - x0, z - z0) };
            let across = (s + 1).min(span - s);
            let along = (r + 1).min(run - r);
            let up = match shape {
                Roof::Flat => 0,
                // Highest over the front, falling to the back.
                Roof::Shed => (z0 + d - z) / (pitch + 1),
                Roof::Gable => across / pitch,
                // A gable whose ends hip back near the ridge.
                Roof::HalfHip => (across / pitch).min(((span + 1) / 2 / pitch / 2).max(1) + along / pitch),
                // The ridge near the front: short and steep before, long and low behind.
                Roof::Saltbox => ((s + 1) / pitch).min((span - s) / (pitch + 1)),
                // Steep low down, low up top.
                Roof::Gambrel => {
                    let knee = ((span + 1) / 4).max(1);
                    if across <= knee { across * 2 } else { knee * 2 + (across - knee) / 2 }
                }
                Roof::Hip => hip(x, z) / pitch,
                Roof::Conical if foot.form == Form::Round => {
                    let (cx, cz) = (x0 + w / 2, z0 + d / 2);
                    let e = (((x - cx).pow(2) + (z - cz).pow(2)) as f32).sqrt();
                    ((w / 2) as f32 + 1.5 - e).max(0.0) as i32
                }
                // Over four walls, a pyramid.
                Roof::Conical => hip(x, z),
            };
            heights.put(x, z, top + 1 + up);
        }
    }
    let gabled = matches!(shape, Roof::Gable | Roof::HalfHip | Roof::Saltbox | Roof::Gambrel);
    // A pitched gable over four walls may be laid in shaped pieces: each column's top a
    // slope whose low edge faces its eave, and a ridge cap along the peak.
    let pieces = if shape == Roof::Gable && pitch == 1 && matches!(foot.form, Form::Box | Form::ShopFront) { mats.pieces(spec.shared_seed()) } else { None };
    let facing = pen.frame.facing;
    let piece = |x: i32, z: i32| -> Option<BlockId> {
        let p = pieces?;
        let s = if ridge_x { z - z0 } else { x - x0 };
        let down = (s + 1).cmp(&(span - s));
        let local = |d: i32| if ridge_x { IVec2::new(0, d) } else { IVec2::new(d, 0) };
        Some(match down {
            std::cmp::Ordering::Less => p.slope[super::roofs::facing_of(turned(facing, local(-1)))],
            std::cmp::Ordering::Greater => p.slope[super::roofs::facing_of(turned(facing, local(1)))],
            // A bare ridge runs east to west, one turned east north to south.
            std::cmp::Ordering::Equal => p.ridge[usize::from(turned(facing, if ridge_x { IVec2::X } else { IVec2::Y }).x == 0)],
        })
    };
    for z in z0 - 1..=z0 + d {
        for x in x0 - 1..=x0 + w {
            let Some(h) = heights.at(x, z) else { continue };
            // Down to one over the lowest beside it, so steep slopes have no holes.
            let mut lo = h;
            for (dx, dz) in SIDES {
                if let Some(n) = heights.at(x + dx, z + dz) {
                    lo = lo.min(n + 1);
                }
            }
            for y in lo..=h {
                pen.set(x, y, z, block);
            }
            if let Some(cap) = piece(x, z) {
                pen.set(x, h, z, cap);
            }
            if !foot.inside(x, z) {
                continue;
            }
            let wall = foot.wall(x, z);
            let (s, r) = if ridge_x { (z - z0, x - x0) } else { (x - x0, z - z0) };
            let gable = gabled && (r == 0 || r == run - 1);
            let joined = ridge_x && ((r == 0 && !open_left) || (r == run - 1 && !open_right));
            let end = gable && !joined && s == span / 2 && blind != Some((x, z));
            // An A-frame's ends are timber, boards in a frame of logs under the roof, so
            // it never reads as a bank of earth whatever its roof.
            let timber = gable && foot.form == Form::AFrame;
            for y in top + 1..lo {
                if !wall {
                    pen.clear(x, y, z);
                } else if end && y >= top + 2 && y < top + 2 + windows && y < lo - 1 {
                    match mats.window {
                        Some(glass) => pen.set(x, y, z, glass),
                        None => pen.clear(x, y, z),
                    }
                } else if timber {
                    pen.set(x, y, z, if y == lo - 1 || s == 0 || s == span - 1 { mats.log } else { mats.trim });
                } else {
                    pen.set(x, y, z, mats.wall);
                }
            }
            if flat && wall && pen.roll(x, top + 2, z, 0x9a2) < 0.85 {
                pen.set(x, top + 2, z, mats.trim);
            }
        }
    }
    // A post on a cone's peak, unless a chimney rises through it (`blind` names the wall
    // behind one), where the two stood side by side like a pair of horns.
    if shape == Roof::Conical
        && blind.is_none()
        && let Some((x, z, y)) = heights.peak()
    {
        pen.set(x, y + 1, z, mats.frame);
    }
    heights
}

/// A step `d` in a building's own frame, in the world.
fn turned(facing: Facing, d: IVec2) -> IVec2 {
    facing.turn(d.x, d.y)
}

fn house(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let (w, d) = (spec.w, spec.d);
    let lift = spec.stilts;
    let foot = Foot { x0: 0, z0: 0, w, d, form: spec.form };
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let mut rng = Rng::new(spec.seed, 0x40e5, 0, 0);
    let top = spec.top() + lift;
    let moss = spec.has(Extra::Moss);
    let canopy = fixed.canopies.pick(unit(hash3(spec.seed, 7, 7, 7))).unwrap_or(mats.trim);
    // The wall block at a place: old walls take moss and cracks here and there.
    let wall_at = |pen: &Pen, x: i32, y: i32, z: i32| -> BlockId {
        if moss && pen.roll(x, y, z, 0x3055) < fixed.moss {
            mats.weathered.at(pen.seed, pen.frame.at(x, y, z)).unwrap_or(mats.wall)
        } else {
            mats.wall
        }
    };

    // Foundation and floor; stilts lift the floor on posts.
    for z in 0..d {
        for x in 0..w {
            if !foot.inside(x, z) {
                continue;
            }
            let edge = foot.wall(x, z);
            if lift > 0 {
                pen.set(x, lift, z, if edge { mats.frame } else { mats.floor });
                for y in 1..lift {
                    pen.clear(x, y, z);
                }
                let post = (x == 0 || x == w - 1 || x == w / 2) && (z == 0 || z == d - 1 || z == d / 2);
                if post || (spec.form != Form::Box && foot.corner(x, z)) {
                    pen.put(x, lift - 1, z, Put::Down(mats.log));
                }
            } else {
                if edge {
                    pen.pal(x, 0, z, &mats.foundation);
                } else {
                    pen.set(x, 0, z, mats.floor);
                }
                let below = mats.foundation.at(spec.seed, frame.at(x, -1, z)).unwrap_or(mats.wall);
                pen.put(x, -1, z, Put::Down(below));
            }
        }
    }

    // Walls, storey by storey, with corner posts and beams between storeys.
    let plinth = mats.wall != mats.frame && rng.chance(fixed.plinth);
    // A stair up each storey but the top, round the inside of the walls.
    let door = spec.door.clamp(1, w - 2);
    let chimney = spec.chimney().filter(|&((x, z), _)| foot.room(x, z));
    let flights = stairs(spec, door, chimney.map(|c| c.0));
    for storey in 0..spec.storeys {
        let base = lift + storey * (spec.wall + 1);
        for y in base + 1..=base + spec.wall {
            for z in 0..d {
                for x in 0..w {
                    if !foot.inside(x, z) {
                        continue;
                    }
                    if !foot.wall(x, z) {
                        pen.clear(x, y, z);
                        continue;
                    }
                    let block = if foot.corner(x, z) {
                        mats.frame
                    } else if plinth && y == lift + 1 && storey == 0 {
                        mats.foundation.at(spec.seed, frame.at(x, y, z)).unwrap_or(mats.wall)
                    } else {
                        wall_at(&pen, x, y, z)
                    };
                    pen.set(x, y, z, block);
                }
            }
        }
        // The floor of the storey above: a beam around, planks within.
        if storey + 1 < spec.storeys {
            let y = base + spec.wall + 1;
            for z in 0..d {
                for x in 0..w {
                    if foot.inside(x, z) {
                        pen.set(x, y, z, if foot.wall(x, z) { mats.frame } else { mats.floor });
                    }
                }
            }
            // The stair: a step up each block from the floor to the storey above, its
            // top three under a hole in that storey's floor, so each is climbed with a
            // jump's headroom (three blocks of air over where it is jumped from).
            if let Some(flight) = flights.get(storey as usize).and_then(|f| f.as_ref()) {
                for (k, &(x, z)) in flight.iter().enumerate().skip(1) {
                    let step = y - k as i32;
                    for up in base + 1..=step {
                        pen.set(x, up, z, mats.floor);
                    }
                    if k <= STAIRWELL {
                        pen.clear(x, y, z);
                    }
                }
            }
        }
        match spec.form {
            Form::Box | Form::ShopFront => windows(spec, &mut pen, mats, base, storey, &mut rng),
            Form::AFrame => {}
            _ => ring_windows(spec, &foot, &mut pen, mats, base, storey, &mut rng),
        }
    }

    // A shop's front: a counter open to the street under an awning.
    if spec.form == Form::ShopFront {
        for x in 1..w - 1 {
            if x == door {
                continue;
            }
            pen.set(x, lift + 1, 0, mats.trim);
            for y in lift + 2..lift + spec.wall {
                pen.clear(x, y, 0);
            }
            if spec.wall >= 3 {
                pen.set(x, lift + spec.wall, 0, mats.frame);
            }
        }
        for x in 0..w {
            pen.set(x, lift + spec.wall + 1, -1, canopy);
            pen.set(x, lift + spec.wall, -2, canopy);
        }
        // Posts at its corners carry the awning's outer edge.
        for x in [0, w - 1] {
            for y in lift + 1..lift + spec.wall {
                pen.set(x, y, -2, mats.frame);
            }
            pen.put(x, lift, -2, Put::Down(mats.frame));
        }
    }

    let gable_windows = if spec.form == Form::AFrame { 3 } else { 1 };
    // A jettied house: its storeys over the ground stand a block out over the street (moved
    // out once the front is drawn), so its roof covers a block more at the front. Not over
    // a porch, an awning or a balcony, nor with a dormer in the roof's front. A house of a
    // row rolls its own, unless its roof slopes down to the street: a jetty moves that eave
    // out, so the whole row shares the roll and the roof runs on at one height.
    let sloped_to_street = spec.ridge_x && spec.roof != Roof::Flat;
    let jetty = (spec.form == Form::Box
        && spec.storeys >= 2
        && ![Extra::Porch, Extra::Awning, Extra::Balcony, Extra::Dormer].iter().any(|&e| spec.has(e)))
    .then(|| mats.jetty(if sloped_to_street { spec.shared_seed() } else { spec.seed }))
    .flatten();
    let covered = if jetty.is_some() { Foot { z0: -1, d: d + 1, ..foot } } else { foot };
    let heights = roof(spec, &mut pen, mats, top, &covered, spec.ridge_x, spec.roof, mats.roof, gable_windows, chimney.map(|c| c.1));

    // The door, a lintel over it, and a step or stair to it; barns, boathouses and inns
    // have wide doors.
    let wide = (spec.kind == Kind::Inn || spec.inside.iter().any(|f| matches!(f, Fitting::Boat | Fitting::Stalls))) && door + 1 < w - 1;
    for dx in 0..=wide as i32 {
        pen.clear(door + dx, lift + 1, 0);
        pen.clear(door + dx, lift + 2, 0);
        if spec.wall >= 3 && spec.form != Form::AFrame {
            pen.set(door + dx, lift + 3, 0, mats.trim);
        }
        pen.ground(door + dx, 1, -1, Put::Clear);
        pen.ground(door + dx, 2, -1, Put::Clear);
        if lift > 0 {
            for step in 1..=lift {
                pen.set(door + dx, step, step - lift - 1, mats.trim);
                pen.put(door + dx, step - 1, step - lift - 1, Put::Down(mats.log));
                pen.clear(door + dx, step + 1, step - lift - 1);
                pen.clear(door + dx, step + 2, step - lift - 1);
            }
        }
    }

    // An A-frame's door in a frame of logs: posts either side, a lintel over.
    if spec.form == Form::AFrame {
        for (x, y) in [(door - 1, lift + 1), (door - 1, lift + 2), (door + 1, lift + 1), (door + 1, lift + 2), (door - 1, lift + 3), (door, lift + 3), (door + 1, lift + 3)] {
            if heights.at(x, 0).is_some_and(|h| y < h) {
                pen.set(x, y, 0, mats.log);
            }
        }
    }

    // Through the front rooms to the court, where something grows.
    if spec.form == Form::Courtyard {
        pen.clear(door, lift + 1, COURT - 1);
        pen.clear(door, lift + 2, COURT - 1);
        let (cx, cz) = (w / 2, d / 2);
        for z in COURT..d - COURT {
            for x in COURT..w - COURT {
                for y in 1..=top - lift + 1 {
                    pen.ground(x, y, z, Put::Clear);
                }
                if (x, z) != (cx, cz)
                    && pen.roll(x, 1, z, 0xc0) < 0.3
                    && let Some(flower) = fixed.flowers.at(spec.seed ^ 0xc1, frame.at(x, 1, z))
                {
                    pen.ground(x, 1, z, Put::Set(flower));
                }
            }
        }
        match mats.fruit.pick(unit(hash3(spec.seed, 0xc2, 0, 0))) {
            Some(leaves) if w - 2 * COURT >= 3 && d - 2 * COURT >= 3 => {
                for up in 1..=3 {
                    pen.ground(cx, up, cz, Put::Set(mats.log));
                }
                for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
                    pen.ground(cx + dx, 4, cz + dz, Put::Set(leaves));
                }
                pen.ground(cx, 5, cz, Put::Set(leaves));
            }
            _ => {
                if let Some(planter) = fixed.planter {
                    pen.ground(cx, 1, cz, Put::Set(planter));
                }
            }
        }
    }

    // A wing behind, on the right, under its own roof.
    if let Some((ww, wd)) = spec.wing {
        let x0 = w - ww;
        let wall = spec.wall.min(3);
        for z in d - 1..d + wd {
            for x in x0..w {
                let edge = x == x0 || x == w - 1 || z == d + wd - 1;
                if z >= d {
                    pen.set(x, lift, z, if edge { mats.frame } else { mats.floor });
                    if lift == 0 {
                        pen.put(x, -1, z, Put::Down(mats.foundation.at(spec.seed, frame.at(x, -1, z)).unwrap_or(mats.wall)));
                    }
                }
                for y in lift + 1..=lift + wall {
                    if z == d - 1 {
                        // Through to the house.
                        if x > x0 && x < w - 1 && y <= lift + 2 {
                            pen.clear(x, y, z);
                        }
                        continue;
                    }
                    if edge {
                        let corner = (x == x0 || x == w - 1) && z == d + wd - 1;
                        pen.set(x, y, z, if corner { mats.frame } else { wall_at(&pen, x, y, z) });
                    } else {
                        pen.clear(x, y, z);
                    }
                }
            }
        }
        if spec.window_chance > 0.0 {
            pen.set(x0 + ww / 2, lift + 2, d + wd - 1, mats.window.unwrap_or(mats.trim));
            if mats.window.is_none() {
                pen.clear(x0 + ww / 2, lift + 2, d + wd - 1);
            }
        }
        // The wing's roof is a plain one of the house's.
        let shape = match spec.roof {
            Roof::HalfHip | Roof::Saltbox | Roof::Gambrel => Roof::Gable,
            Roof::Conical => Roof::Hip,
            other => other,
        };
        roof(spec, &mut pen, mats, lift + wall, &Foot::boxed(x0, d, ww, wd), false, shape, mats.roof, 1, None);
    }

    // A lean-to shed on the right.
    if spec.shed > 0 {
        let z0 = (d - spec.shed) / 2;
        for z in z0..z0 + spec.shed {
            for x in w..w + 2 {
                pen.set(x, lift, z, mats.floor);
                pen.put(x, lift - 1, z, Put::Down(mats.foundation.at(spec.seed, frame.at(x, -1, z)).unwrap_or(mats.wall)));
                let edge = x == w + 1 || z == z0 || z == z0 + spec.shed - 1;
                for y in lift + 1..=lift + 2 {
                    if edge {
                        pen.set(x, y, z, if (z == z0 || z == z0 + spec.shed - 1) && x == w + 1 { mats.frame } else { mats.trim });
                    } else {
                        pen.clear(x, y, z);
                    }
                }
                pen.set(x, lift + 3, z, mats.roof);
            }
            pen.set(w + 2, lift + 3, z, mats.roof);
        }
        // Its door, from the side.
        pen.clear(w + 1, lift + 1, z0 + spec.shed / 2);
        pen.clear(w + 1, lift + 2, z0 + spec.shed / 2);
        if spec.shed >= 3 {
            pen.set(w, lift + 1, z0 + 1, fixed.table);
        }
    }

    // A hearth lights the house, its chimney drawn last, over all within.
    let mut lit = chimney.is_some();

    // Inside: a working building's fittings, or a table, a light and sometimes a planter.
    // The first free floor near a place.
    let free = |x: i32, z: i32| -> (i32, i32) {
        let stair = |x: i32, z: i32| flights.first().and_then(|f| f.as_ref()).is_some_and(|f| f[1..].contains(&(x, z)));
        let ok = |x: i32, z: i32| foot.room(x, z) && !(x == door && z <= 1) && !stair(x, z) && chimney.is_none_or(|c| c.0 != (x, z));
        if ok(x, z) {
            return (x, z);
        }
        for r in 1..w.max(d) {
            for dz in -r..=r {
                for dx in -r..=r {
                    if ok(x + dx, z + dz) {
                        return (x + dx, z + dz);
                    }
                }
            }
        }
        (x, z)
    };
    if !spec.inside.is_empty() {
        fittings(spec, &mut pen, mats, fixed, lift, door);
        if !lit {
            let (x, z) = free(1, 1);
            pen.set(x, lift + 1, z, fixed.torch);
            lit = true;
        }
    } else if d >= 5 && w >= 4 && !(spec.kind == Kind::House && fixed.interiors.is_some()) {
        // A home with interiors is furnished once its people are known (`interior`).
        let (tx, tz) = if rng.chance(0.5) { (w - 2, d - 2) } else { (w - 2, 1) };
        let (tx, tz) = if tx == door && tz == 1 { (1, d - 2) } else { (tx, tz) };
        let (tx, tz) = free(tx, tz);
        if !(spec.shed > 0 && tx == w - 2) || rng.chance(0.5) {
            pen.set(tx, lift + 1, tz, fixed.table);
            if !lit {
                pen.set(tx, lift + 2, tz, fixed.torch);
                lit = true;
            }
        }
        if let Some(planter) = fixed.planter
            && rng.chance(0.3)
        {
            let (px, pz) = free(1, d - 2);
            pen.set(px, lift + 1, pz, planter);
        }
    }
    if !lit {
        let (x, z) = free(1, 1);
        pen.set(x, lift + 1, z, fixed.torch);
    }
    if spec.kind == Kind::Inn {
        // Tables down the room.
        for z in (2..d - 2).step_by(3) {
            if w >= 7 {
                pen.set(w / 2, lift + 1, z, fixed.table);
            }
        }
    }
    // The chimney: a hearth on the floor (or the oven), and one block of the style's
    // chimney stone up through the floors and the roof, one or two over the roof there.
    if let Some(((x, z), _)) = chimney
        && let Some(roof) = heights.at(x, z)
    {
        let from = if spec.inside.contains(&Fitting::Oven) {
            lift + 3
        } else {
            pen.set(x, lift + 1, z, fixed.campfire);
            lift + 2
        };
        // Clear of the roof beside it too, a slope or a wing's eaves.
        let beside = SIDES.iter().filter_map(|&(dx, dz)| pen.highest(x + dx, z + dz, roof, roof + 4)).fold(roof, i32::max);
        let over = if beside == roof && pen.roll(x, roof, z, 0xc41) < 0.5 { 1 } else { 0 };
        for y in from..=beside + 1 + over {
            pen.set(x, y, z, mats.chimney);
        }
    }

    // Outside: porch, lamp, flower boxes, woodpile, the trader's counter.
    if spec.has(Extra::Porch) {
        for z in -3..0 {
            for x in 0..w {
                pen.set(x, lift, z, mats.trim);
                pen.put(x, lift - 1, z, Put::Down(mats.log));
                for y in lift + 1..=lift + spec.wall {
                    pen.clear(x, y, z);
                }
            }
        }
        for x in [0, w - 1] {
            for y in lift + 1..=lift + spec.wall {
                pen.set(x, y, -3, mats.frame);
            }
        }
        for z in -4..0 {
            for x in -1..=w {
                pen.set(x, lift + spec.wall + 1, z, mats.roof);
            }
        }
    }
    if spec.kind == Kind::Trader && spec.form != Form::ShopFront {
        let (a, b) = (1, w - 2);
        for x in a..=b {
            if x != door {
                pen.set(x, lift + 1, -2, mats.trim);
            }
            pen.set(x, lift + 4.min(spec.wall + 1), -3, canopy);
            pen.set(x, lift + 4.min(spec.wall + 1), -2, canopy);
            pen.set(x, lift + 4.min(spec.wall + 1), -1, canopy);
        }
        for x in [a - 1, b + 1] {
            for y in lift + 1..lift + 4.min(spec.wall + 1) {
                pen.set(x, y, -3, mats.frame);
            }
            pen.put(x, lift, -3, Put::Down(mats.log));
        }
        pen.set(b, lift + 2, -2, fixed.torch);
    }
    let balcony = spec.has(Extra::Balcony) && spec.storeys >= 2 && !spec.has(Extra::Porch);
    if (spec.has(Extra::Lamp) || spec.kind == Kind::Inn) && !balcony {
        let sides: &[i32] = if spec.kind == Kind::Inn { &[-1, 2] } else { &[if rng.chance(0.5) { -1 } else { 1 }] };
        for &side in sides {
            let x = (door + side).clamp(-1, w);
            let x = if x == door { door - 1 } else { x };
            let z = if spec.has(Extra::Porch) {
                -4
            } else if spec.form == Form::ShopFront {
                -3
            } else {
                -2
            };
            lamp(&mut pen, x, z, lift, mats.frame, fixed.torch);
        }
    }
    if spec.has(Extra::FlowerBoxes) && lift == 0 {
        for x in 0..w {
            let Some(zf) = (0..d).find(|&z| foot.inside(x, z)) else { continue };
            if (x - door).abs() >= 2 && !foot.corner(x, zf) && pen.roll(x, 1, zf - 1, 0xf10) < 0.75 {
                pen.ground(x, 1, zf - 1, Put::Soft(mats.trim));
                if let Some(flower) = fixed.flowers.at(spec.seed ^ 0xf1, frame.at(x, 2, zf - 1)) {
                    pen.ground(x, 2, zf - 1, Put::Soft(flower));
                }
            }
        }
    }
    if spec.has(Extra::Woodpile) && lift == 0 {
        // Against the left wall, logs stacked two high.
        let z0 = 1.max(d / 2 - 1);
        for z in z0..(z0 + 3).min(d - 1) {
            pen.ground(-1, 1, z, Put::Soft(mats.log));
            if pen.roll(-1, 2, z, 0x3d) < 0.6 {
                pen.ground(-1, 2, z, Put::Soft(mats.log));
            }
        }
    }

    // Dormers stand out of the front slope of a roof whose ridge runs across the front.
    let gabled = matches!(spec.roof, Roof::Gable | Roof::HalfHip | Roof::Saltbox | Roof::Gambrel);
    if spec.has(Extra::Dormer) && gabled && spec.ridge_x && d >= 6 && matches!(spec.form, Form::Box | Form::ShopFront) {
        let at: &[i32] = if w >= 9 { &[w / 3, w - 1 - w / 3] } else { &[w / 2] };
        let y0 = top + 2;
        for &x in at {
            let Some(zf) = (0..d / 2).find(|&z| heights.at(x, z).is_some_and(|h| h >= y0)) else { continue };
            for dx in -1..=1 {
                for z in zf - 1..=zf + 2 {
                    if heights.at(x + dx, z).is_some_and(|h| h <= y0 + 1) {
                        pen.set(x + dx, y0 + 1, z, mats.roof);
                    }
                }
            }
            pen.set(x - 1, y0, zf, mats.wall);
            pen.set(x + 1, y0, zf, mats.wall);
            match mats.window {
                Some(glass) => pen.set(x, y0, zf, glass),
                None => pen.clear(x, y0, zf),
            }
            for z in zf + 1..zf + 3 {
                if heights.at(x, z).is_some_and(|h| h <= y0) {
                    pen.clear(x, y0, z);
                }
            }
        }
    }
    // A balcony over the door, on the floor above, on two posts.
    if balcony {
        let b = lift + spec.wall + 1;
        for x in door - 2..=door + 2 {
            for z in -2..=-1 {
                pen.set(x, b, z, mats.trim);
                pen.clear(x, b + 2, z);
            }
            pen.set(x, b + 1, -2, if x == door - 2 || x == door + 2 { mats.frame } else { mats.trim });
        }
        for x in [door - 2, door + 2] {
            pen.set(x, b + 1, -1, mats.frame);
            pen.put(x, b - 1, -2, Put::Down(mats.frame));
        }
        pen.clear(door, b + 1, 0);
        pen.clear(door, b + 2, 0);
        pen.clear(door, b + 1, -1);
    }
    // A stair up the right side to a door on the floor above.
    let stair = spec.has(Extra::OutsideStair) && spec.storeys >= 2 && spec.shed == 0 && d >= spec.wall + 4;
    if stair {
        let n = spec.wall + 1;
        for k in 1..=n + 1 {
            let z = k;
            let y = lift + k.min(n);
            pen.set(w, y, z, mats.trim);
            pen.put(w, y - 1, z, Put::Down(mats.trim));
            for up in 1..=3 {
                pen.clear(w, y + up, z);
            }
        }
        pen.clear(w - 1, lift + n + 1, n + 1);
        pen.clear(w - 1, lift + n + 2, n + 1);
    }
    // Shutters beside the windows are drawn with them; an awning over the front windows.
    if spec.has(Extra::Awning) && spec.form == Form::Box && !spec.has(Extra::Porch) && spec.kind != Kind::Trader && spec.wall >= 3 {
        for x in 1..w - 1 {
            pen.set(x, lift + spec.wall, -1, canopy);
        }
    }
    // A sign by the door.
    if spec.has(Extra::SignBoard) && spec.wall >= 3 && spec.form != Form::AFrame {
        let x = if door + 1 < w - 1 { door + 1 } else { door - 1 };
        let board = fixed.canopies.pick(unit(hash3(spec.seed, 5, 1, 9))).unwrap_or(mats.trim);
        pen.set(x, lift + 3, -1, board);
    }
    // A barrel at a front corner.
    if spec.has(Extra::RainBarrel)
        && lift == 0
        && let Some(barrel) = fixed.barrel
    {
        let x = if stair || spec.shed > 0 { -1 } else { w };
        pen.ground(x, 1, 0, Put::Soft(barrel));
    }
    // A vane on the peak.
    if spec.has(Extra::Weathervane)
        && spec.roof != Roof::Flat
        && let Some((x, z, y)) = heights.peak()
    {
        pen.set(x, y + 1, z, mats.frame);
        pen.set(x, y + 2, z, mats.trim);
    }
    // A dovecote on a pole behind.
    if spec.has(Extra::Dovecote) {
        let x = if spec.wing.is_some() { 1 } else { w - 2 };
        for up in 1..=3 {
            pen.ground(x, up, d, Put::Set(mats.frame));
        }
        pen.ground(x, 4, d, Put::Set(mats.trim));
        pen.ground(x, 5, d, Put::Set(mats.roof));
    }
    // Herbs hung under the front eaves.
    if spec.has(Extra::HangingHerbs) && spec.wall >= 3 {
        for x in 0..w {
            if (x - door).abs() >= 1
                && heights.at(x, -1) == Some(top + 1)
                && pen.roll(x, top, -1, 0x4e7) < 0.45
                && let Some(herbs) = fixed.hanging.at(spec.seed ^ 0x4e8, frame.at(x, top, -1))
            {
                pen.set(x, top, -1, herbs);
            }
        }
    }
    if let Some(beams) = jetty {
        jettied(&mut pen, spec, mats, lift, top, beams);
    }
}

/// Moves the front of a house's storeys over the ground a block out, with what hangs on
/// it (shutters, flower boxes), the floor of the first carried on a row of jetty `beams`
/// whose braces rest on the wall below; the old front of each storey becomes room.
fn jettied(pen: &mut Pen, spec: &Spec, mats: &Mats, lift: i32, top: i32, beams: [BlockId; 4]) {
    let w = spec.w;
    let floor = lift + spec.wall + 1;
    // Not a neighbour's in its row: its own front moves out with it.
    let (from_x, to_x) = spec.row.map_or((-1, w), |r| (if r.left { 0 } else { -1 }, if r.right { w - 1 } else { w }));
    for y in floor..=top {
        for z in [-1, 0] {
            for x in from_x..=to_x {
                let (from, to) = (pen.frame.at(x, y, z), pen.frame.at(x, y, z - 1));
                match pen.canvas.absolute.get(&from).copied() {
                    Some(put) => pen.canvas.put(to, put),
                    None => {
                        pen.canvas.absolute.remove(&to);
                    }
                }
                // Where the front wall stood, each storey's floor runs on to the wall's
                // new line; the rest is room.
                let storey_floor = (y - lift) % (spec.wall + 1) == 0 && (y - lift) / (spec.wall + 1) < spec.storeys;
                if z == 0 && x > 0 && x < w - 1 {
                    if storey_floor { pen.set(x, y, 0, mats.floor) } else { pen.clear(x, y, 0) }
                } else if z == 0 && (x < 0 || x >= w) {
                    pen.canvas.absolute.remove(&from);
                }
            }
        }
    }
    let out = super::roofs::facing_of(turned(pen.frame.facing, IVec2::new(0, -1)));
    // Under the overhang, but never over what already hangs there (a sign board at the
    // top of walls of three).
    for x in 0..w {
        if !pen.canvas.absolute.contains_key(&pen.frame.at(x, floor - 1, -1)) {
            pen.set(x, floor - 1, -1, beams[out]);
        }
    }
}

/// What stands inside a working building, on its ground floor.
/// Each storey's stair but the top's: `wall + 1` cells in a run round the inside of the
/// walls, the first in the floor of the storey above (stepped up onto), the rest steps
/// down to the floor. Clear of the door, the hearth, the shop's counter and the top of the
/// stair below; for each storey, none where no run fits (only four-walled houses have
/// them).
/// A house too small for both its chimney and a stair up each storey goes without the
/// chimney, one too small for its stairwells with tall walls has walls of three, and one
/// too small for a stair even so (a mod's narrow house) has fewer storeys, down to one.
pub fn make_room_for_stairs(spec: &mut Spec) {
    let fits = |spec: &Spec| !stairs(spec, spec.door.clamp(1, spec.w - 2), spec.chimney().map(|c| c.0)).iter().any(Option::is_none);
    // The same specification has the same flights. Only its mutations invalidate
    // the fit, so successful chimney/wall checks need not build those paths again.
    let mut fit = None;
    if spec.chimney().is_some() && !*fit.get_or_insert_with(|| fits(spec)) {
        spec.extras.retain(|e| *e != Extra::Chimney);
        fit = None;
    }
    while spec.wall > 3 && !*fit.get_or_insert_with(|| fits(spec)) {
        spec.wall -= 1;
        fit = None;
    }
    while spec.storeys > 1 && !*fit.get_or_insert_with(|| fits(spec)) {
        spec.storeys -= 1;
        fit = None;
    }
}

/// Steps of a stair under the hole in the floor above: a player jumps a block with three
/// blocks of air overhead, so every step within three of the floor above opens to it.
const STAIRWELL: usize = 3;

/// The stairs `house` draws for `spec`, as `stairs` gives them.
pub(super) fn flights(spec: &Spec) -> Vec<Option<Vec<(i32, i32)>>> {
    let foot = Foot { x0: 0, z0: 0, w: spec.w, d: spec.d, form: spec.form };
    let chimney = spec.chimney().filter(|&((x, z), _)| foot.room(x, z));
    stairs(spec, spec.door.clamp(1, spec.w - 2), chimney.map(|c| c.0))
}

pub(super) fn stairs(spec: &Spec, door: i32, hearth: Option<(i32, i32)>) -> Vec<Option<Vec<(i32, i32)>>> {
    let (w, d) = (spec.w, spec.d);
    let run = (spec.wall + 1) as usize;
    if spec.storeys < 2 || !matches!(spec.form, Form::Box | Form::ShopFront) {
        return Vec::new();
    }
    // Too narrow for a run round the inside: a storey above with no way up.
    if w < 4 || d < 4 {
        return vec![None; (spec.storeys - 1) as usize];
    }
    // The cells inside the walls, round in order: along the back, down the left, along the
    // front and up the right.
    let (x0, x1, z0, z1) = (1, w - 2, 1, d - 2);
    let mut ring: Vec<(i32, i32)> = Vec::new();
    ring.extend((x0..=x1).rev().map(|x| (x, z1)));
    ring.extend((z0..z1).rev().map(|z| (x0, z)));
    ring.extend((x0 + 1..=x1).map(|x| (x, z0)));
    ring.extend((z0 + 1..z1).map(|z| (x1, z)));
    ring.dedup();
    if ring.len() < run + 1 {
        return vec![None; (spec.storeys - 1) as usize];
    }
    let mut flights = Vec::new();
    let mut below: Vec<(i32, i32)> = Vec::new();
    // An inn's door is two wide.
    let wide = if spec.kind == Kind::Inn { 1 } else { 0 };
    for storey in 0..spec.storeys - 1 {
        // Clear of the way in and the shop's counter on the ground floor, of the chimney's
        // flue on every floor, and of the holes the flight below comes up through.
        let banned = |&(x, z): &(i32, i32)| {
            let ground = storey == 0 && z <= 2 && ((x >= door && x <= door + wide) || spec.form == Form::ShopFront);
            ground || Some((x, z)) == hearth || below.contains(&(x, z))
        };
        let n = ring.len();
        let found = (0..n).flat_map(|start| [1isize, -1].map(|dir| (start, dir))).find_map(|(start, dir)| {
            let at = |k| ring[(start as isize + dir * k as isize).rem_euclid(n as isize) as usize];
            (0..run).all(|k| !banned(&at(k))).then(|| (0..run).map(at).collect::<Vec<_>>())
        });
        below = found.as_ref().map_or_else(Vec::new, |f| f[..(STAIRWELL + 1).min(f.len())].to_vec());
        flights.push(found);
    }
    flights
}

fn fittings(spec: &Spec, pen: &mut Pen, mats: &Mats, fixed: &Fixed, lift: i32, door: i32) {
    let (w, d) = (spec.w, spec.d);
    let y = lift + 1;
    let (cx, cz) = (w / 2, d / 2);
    let stone = mats.stone();
    let wall_top = lift + spec.wall;
    for &fitting in &spec.inside {
        match fitting {
            Fitting::Oven => {
                // Stone round a fire, against the back wall on the left.
                for x in 1..=3.min(w - 2) {
                    pen.set(x, y + 1, d - 2, stone);
                    pen.set(x, y, d - 2, if x == 2 { fixed.campfire } else { stone });
                }
            }
            Fitting::Millstone => {
                for (x, z) in [(cx, cz), (cx - 1, cz), (cx, cz - 1), (cx - 1, cz - 1)] {
                    pen.set(x, y, z, stone);
                }
                for yy in y + 1..=wall_top {
                    pen.set(cx, yy, cz, mats.log);
                }
            }
            Fitting::Vats => {
                if lift == 0 {
                    for z in (2..d - 2).step_by(3) {
                        for x in (2..w - 2).step_by(3) {
                            if x != door {
                                pen.set(x, 0, z, fixed.water);
                            }
                        }
                    }
                }
            }
            Fitting::Racks => {
                if spec.wall >= 3 {
                    for z in (2..d - 1).step_by(2) {
                        for x in [1, w - 2] {
                            pen.set(x, wall_top, z, mats.trim);
                            if let Some(h) = fixed.hanging.at(spec.seed ^ 0x7ac, pen.frame.at(x, wall_top - 1, z)) {
                                pen.set(x, wall_top - 1, z, h);
                            }
                        }
                    }
                }
            }
            Fitting::Stalls => {
                for z in (2..d - 1).step_by(3) {
                    for x in [2, w - 3] {
                        pen.set(x, y, z, mats.frame);
                    }
                    for x in [1, w - 2] {
                        if let Some(hay) = fixed.hay.at(spec.seed ^ 0x4a, pen.frame.at(x, y, z + 1)) {
                            pen.set(x, y, (z + 1).min(d - 2), hay);
                        }
                    }
                }
            }
            Fitting::Bins => {
                for x in 1..w - 1 {
                    pen.set(x, y, d - 2, mats.trim);
                    if pen.roll(x, y + 1, d - 2, 0xb1) < 0.6 {
                        pen.set(x, y + 1, d - 2, mats.trim);
                    }
                    if d >= 6
                        && pen.roll(x, y, d - 3, 0xb2) < 0.4
                        && let Some(sack) = fixed.sacks.at(spec.seed ^ 0xb3, pen.frame.at(x, y, d - 3))
                    {
                        pen.set(x, y, d - 3, sack);
                    }
                }
            }
            Fitting::Smoke => {
                pen.set(cx, y, cz, fixed.campfire);
                for x in 1..w - 1 {
                    for z in [cz - 1, cz + 1] {
                        pen.set(x, wall_top, z, mats.trim);
                        if x != cx
                            && let Some(h) = fixed.hanging.at(spec.seed ^ 0x5e0, pen.frame.at(x, wall_top - 1, z))
                        {
                            pen.set(x, wall_top - 1, z, h);
                        }
                    }
                }
            }
            Fitting::Altar => {
                pen.set(cx, y, d - 2, fixed.altar.unwrap_or(stone));
                let off = if w >= 7 { 2 } else { 1 };
                for x in [cx - off, cx + off] {
                    pen.set(x, y, d - 2, mats.frame);
                    pen.set(x, y + 1, d - 2, fixed.torch);
                }
            }
            Fitting::Pews => {
                for z in (3..d - 3).step_by(2) {
                    for x in 1..w - 1 {
                        if (x - cx).abs() >= 1 && x != door {
                            pen.set(x, y, z, mats.trim);
                        }
                    }
                }
            }
            Fitting::Boat => {
                // A hull on the floor, bow to the door.
                for z in 2..(d - 1).min(7) {
                    for x in [cx - 1, cx + 1] {
                        pen.set(x, y, z, mats.trim);
                    }
                }
                pen.set(cx, y, 2, mats.trim);
                pen.set(cx, y, (d - 2).min(6), mats.trim);
            }
            Fitting::Counter => {
                let z = d - 3;
                for x in 1..w - 2 {
                    pen.set(x, y, z, mats.trim);
                }
            }
        }
    }
}

/// A lamp post: a post with a torch on top.
fn lamp(pen: &mut Pen, x: i32, z: i32, lift: i32, post: BlockId, torch: BlockId) {
    let _ = lift;
    pen.ground(x, 1, z, Put::Set(post));
    pen.ground(x, 2, z, Put::Set(post));
    pen.ground(x, 3, z, Put::Set(torch));
}

/// Windows on every wall of one storey of four walls, with shutters where it has them.
fn windows(spec: &Spec, pen: &mut Pen, mats: &Mats, base: i32, storey: i32, rng: &mut Rng) {
    let (w, d) = (spec.w, spec.d);
    let y = base + if spec.wall >= 3 { 2 } else { 1 };
    let chance = spec.window_chance;
    let phase = rng.below(2);
    let mut placed: Vec<(i32, i32)> = Vec::new();
    let mut place = |pen: &mut Pen, x: i32, z: i32, along: i32, rng: &mut Rng| {
        let wanted = if spec.regular { (along + phase) % 2 == 0 && rng.chance(chance + 0.3) } else { rng.chance(chance * 0.7) };
        if !wanted || spec.chimney().is_some_and(|c| c.1 == (x, z)) {
            return;
        }
        placed.push((x, z));
        match mats.window {
            Some(glass) => pen.set(x, y, z, glass),
            None => pen.clear(x, y, z),
        }
        // A taller window in the walls of grander houses.
        if spec.wall >= 4 && rng.chance(0.5) {
            match mats.window {
                Some(glass) => pen.set(x, y + 1, z, glass),
                None => pen.clear(x, y + 1, z),
            }
        }
    };
    for x in 1..w - 1 {
        // Not right beside the door.
        if !(storey == 0 && (x - spec.door).abs() <= 1) {
            place(pen, x, 0, x, rng);
        }
        if !(spec.wing.is_some_and(|(ww, _)| x >= w - ww) && storey == 0) {
            place(pen, x, d - 1, x, rng);
        }
    }
    let (left, right) = spec.row.map_or((false, false), |r| (r.left, r.right));
    for z in 1..d - 1 {
        if !left {
            place(pen, 0, z, z, rng);
        }
        if !(spec.shed > 0 && storey == 0) && !right {
            place(pen, w - 1, z, z, rng);
        }
    }
    if spec.has(Extra::Shutters) && mats.window.is_some() {
        for &(x, z) in &placed {
            let along: [(i32, i32); 2] = if z == 0 || z == d - 1 { [(x - 1, z), (x + 1, z)] } else { [(x, z - 1), (x, z + 1)] };
            for (sx, sz) in along {
                let corner = (sx == 0 || sx == w - 1) && (sz == 0 || sz == d - 1);
                let by_door = storey == 0 && sz == 0 && sx == spec.door;
                if !corner && !by_door && !placed.contains(&(sx, sz)) {
                    pen.set(sx, y, sz, mats.trim);
                }
            }
        }
    }
}

/// Windows round the walls of a house of another form, by chance.
#[allow(clippy::too_many_arguments)]
fn ring_windows(spec: &Spec, foot: &Foot, pen: &mut Pen, mats: &Mats, base: i32, storey: i32, rng: &mut Rng) {
    let y = base + if spec.wall >= 3 { 2 } else { 1 };
    for z in 0..spec.d {
        for x in 0..spec.w {
            if !foot.wall(x, z) || foot.corner(x, z) {
                continue;
            }
            if storey == 0 && (x - spec.door).abs() <= 1 && z <= COURT {
                continue;
            }
            if spec.chimney().is_some_and(|c| c.1 == (x, z)) {
                continue;
            }
            if rng.chance(spec.window_chance * 0.6) {
                match mats.window {
                    Some(glass) => pen.set(x, y, z, glass),
                    None => pen.clear(x, y, z),
                }
            }
        }
    }
}

/// An open-fronted smithy: a stone floor, a back wall, posts and a roof, a forge of stone
/// round a fire, an anvil and a trough.
fn smithy(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let (w, d) = (spec.w, spec.d);
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    for z in 0..d {
        for x in 0..w {
            pen.pal(x, 0, z, &mats.foundation);
            pen.put(x, -1, z, Put::Down(mats.foundation.0.first().map_or(mats.wall, |b| b.0)));
            for y in 1..=spec.wall {
                let back = z == d - 1;
                let side = (x == 0 || x == w - 1) && z >= d / 2;
                let post = (x == 0 || x == w - 1) && (z == 0 || z == d - 1);
                if post {
                    pen.set(x, y, z, mats.frame);
                } else if back || side {
                    pen.set(x, y, z, mats.wall);
                } else {
                    pen.clear(x, y, z);
                }
            }
        }
    }
    roof(spec, &mut pen, mats, spec.wall, &Foot::boxed(0, 0, w, d), spec.ridge_x, spec.roof, mats.roof, 0, None);
    // The forge in a back corner.
    let stone = mats.stone();
    for (x, z) in [(1, d - 2), (2, d - 2), (1, d - 3)] {
        pen.set(x, 1, z, stone);
    }
    pen.set(2, 1, d - 3, fixed.campfire);
    pen.set(1, 2, d - 2, stone);
    // Anvil and trough.
    pen.set(w - 3, 1, d / 2, fixed.anvil);
    pen.set(w - 2, 1, 1, stone);
    pen.set(w - 2, 0, 1, fixed.water);
    lamp(&mut pen, -1, -1, 0, mats.frame, fixed.torch);
}

/// A market stall: a counter, four posts and a cloth of colour.
fn stall(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let canopy = fixed.canopies.pick(unit(hash3(spec.seed, 1, 2, 3))).unwrap_or(mats.trim);
    let (w, d) = (spec.w, spec.d);
    for x in 0..w {
        for z in 0..d {
            for y in 1..=3 {
                pen.ground(x, y, z, Put::Clear);
            }
            pen.ground(x, 4, z, Put::Set(canopy));
        }
        if x > 0 && x < w - 1 {
            pen.ground(x, 1, 0, Put::Set(mats.trim));
        }
    }
    for (x, z) in [(0, 0), (w - 1, 0), (0, d - 1), (w - 1, d - 1)] {
        for y in 1..=3 {
            pen.ground(x, y, z, Put::Set(mats.frame));
        }
    }
    // Goods: a crate behind the counter.
    if d >= 3 {
        pen.ground(1, 1, d - 1, Put::Set(mats.log));
    }
    let _ = fixed;
}

/// A well: a ring of stone round water, two posts and a little roof.
fn well(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let (w, d) = (spec.w, spec.d);
    for z in 0..d {
        for x in 0..w {
            let edge = x == 0 || z == 0 || x == w - 1 || z == d - 1;
            if edge {
                pen.pal(x, 0, z, &mats.foundation);
                pen.pal(x, 1, z, &mats.foundation);
                for y in -4..0 {
                    pen.pal(x, y, z, &mats.foundation);
                }
            } else {
                for y in -4..=0 {
                    pen.set(x, y, z, fixed.water);
                }
                pen.pal(x, -5, z, &mats.foundation);
                pen.clear(x, 1, z);
            }
            for y in 2..=3 {
                pen.clear(x, y, z);
            }
        }
    }
    for (x, z) in [(0, d / 2), (w - 1, d / 2)] {
        pen.set(x, 2, z, mats.frame);
        pen.set(x, 3, z, mats.frame);
    }
    for z in 0..d {
        for x in 0..w {
            if spec.roof == Roof::Flat || (z - d / 2).abs() <= 1 {
                pen.set(x, 4, z, mats.roof);
            }
        }
    }
}

/// A wayside shrine: a stepped plinth, an altar, and a pillar and a light behind it.
fn shrine(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let (w, d) = (spec.w, spec.d);
    for z in 0..d {
        for x in 0..w {
            pen.ground(x, 0, z, Put::Set(mats.foundation.at(spec.seed, frame.at(x, 0, z)).unwrap_or(mats.wall)));
            pen.ground(x, 1, z, Put::Clear);
            pen.ground(x, 2, z, Put::Clear);
        }
    }
    let (cx, cz) = (w / 2, d / 2);
    // The altar in the middle and its lamp behind it; without one (or the room), the lamp
    // alone.
    let lamp = match fixed.altar {
        Some(altar) if d - 1 > cz => {
            pen.ground(cx, 1, cz, Put::Set(altar));
            d - 1
        }
        _ => cz,
    };
    pen.ground(cx, 1, lamp, Put::Set(mats.frame));
    pen.ground(cx, 2, lamp, Put::Set(mats.frame));
    pen.ground(cx, 3, lamp, Put::Set(fixed.torch));
    for (x, z) in [(cx - 1, cz), (cx + 1, cz)] {
        if let Some(flower) = fixed.flowers.at(spec.seed ^ 0x5a, frame.at(x, 1, z)) {
            pen.ground(x, 1, z, Put::Soft(flower));
        }
    }
}

/// A field of one crop in rows, a ditch of water through the middle and a border of
/// logs.
fn field(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let (w, d) = (spec.w, spec.d);
    let mut rng = Rng::new(spec.seed, 0xf1e1d, 0, 0);
    let total: f32 = mats.crops.iter().map(|c| c.1).sum();
    let mut roll = rng.unit() * total;
    let crop = mats.crops.iter().find(|c| {
        roll -= c.1;
        roll <= 0.0
    }).or(mats.crops.last()).map(|c| &c.0[..]);
    // How far grown the whole field is, give or take.
    let growth = rng.unit();
    let ditch = w / 2;
    for z in 0..d {
        for x in 0..w {
            let edge = x == 0 || z == 0 || x == w - 1 || z == d - 1;
            if edge {
                pen.set(x, 0, z, mats.log);
                pen.put(x, -1, z, Put::Down(fixed.dirt));
                pen.clear(x, 1, z);
                continue;
            }
            if x == ditch {
                pen.set(x, 0, z, fixed.water);
                pen.set(x, -1, z, fixed.dirt);
                pen.clear(x, 1, z);
                continue;
            }
            pen.set(x, 0, z, fixed.farmland);
            pen.put(x, -1, z, Put::Down(fixed.dirt));
            match crop {
                Some(stages) => {
                    // Growing, never ripe: what a field gives is its grower's. Each of the
                    // growing stages (all but the last) by how far grown the field is.
                    let last = (stages.len() - 2) as f32;
                    let u = ((growth * 2.5 + rng.unit() * 1.2 - 0.4) * last / 2.0).clamp(0.0, last) as usize;
                    pen.set(x, 1, z, stages[u]);
                }
                None => pen.clear(x, 1, z),
            }
            pen.clear(x, 2, z);
        }
    }
}

/// A pen: a hedge or log rail round open grass, with a gap for a gate.
fn pen(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let (w, d) = (spec.w, spec.d);
    let rail = !mats.hedge.is_empty() && hash3(spec.seed, 0, 0, 0) % 3 == 0;
    let gate = w / 2;
    for z in 0..d {
        for x in 0..w {
            let edge = x == 0 || z == 0 || x == w - 1 || z == d - 1;
            if edge && !(z == 0 && (x == gate || x == gate + 1)) {
                let corner = (x == 0 || x == w - 1) && (z == 0 || z == d - 1);
                let block = if rail && !corner { mats.hedge.at(spec.seed, frame.at(x, 1, z)).unwrap_or(mats.log) } else { mats.log };
                pen.ground(x, 1, z, Put::Set(block));
            } else {
                pen.ground(x, 1, z, Put::Clear);
            }
        }
    }
    // A trough.
    pen.ground(w - 2, 0, d - 2, Put::Set(fixed.water));
    pen.ground(w - 3, 1, d - 2, Put::Soft(mats.trim));
}

/// An orchard: small fruit trees in rows on grass.
fn orchard(spec: &Spec, frame: &Frame, mats: &Mats, _fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let (w, d) = (spec.w, spec.d);
    let leaves = mats.fruit.pick(unit(hash3(spec.seed, 3, 1, 4))).unwrap_or(mats.trim);
    for z in (1..d - 1).step_by(4) {
        for x in (1..w - 1).step_by(4) {
            if pen.roll(x, 0, z, 0x0c4) < 0.15 {
                continue;
            }
            pen.ground(x, 1, z, Put::Set(mats.log));
            pen.ground(x, 2, z, Put::Set(mats.log));
            for dz in -1..=1 {
                for dx in -1..=1 {
                    for up in 3..=4 {
                        let corner = dx != 0 && dz != 0;
                        if corner && (up == 4 || pen.roll(x + dx, up, z + dz, 0x1ea) < 0.5) {
                            continue;
                        }
                        pen.ground(x + dx, up, z + dz, Put::Soft(leaves));
                    }
                }
            }
            pen.ground(x, 3, z, Put::Set(mats.log));
        }
    }
}

/// A woodyard: stacks of logs and a chopping block.
fn woodyard(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let (w, d) = (spec.w, spec.d);
    for z in 0..d {
        for x in 0..w {
            let stack = z % 3 != 2 && x < w - 2;
            if stack {
                let high = 1 + (pen.roll(x, 0, z, 0x7d) * 2.5) as i32;
                for up in 1..=high {
                    pen.ground(x, up, z, Put::Soft(mats.log));
                }
            }
        }
    }
    pen.ground(w - 1, 1, d - 1, Put::Soft(mats.log));
    let _ = fixed;
}
