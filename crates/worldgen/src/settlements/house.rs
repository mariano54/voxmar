//! Houses and the other buildings of a settlement, drawn block by block in their own
//! coordinates: the front at z = 0 looking towards −z, x across the front from 0 to
//! `w − 1`, z to the back at `d − 1`, the floor at y = 0.
//!
//! Every house is rolled: its kind (`houses` in settlements.ron) sets its size, walls and
//! storeys; its roof is the style's (gable, hipped, flat or a single slope, steep or low);
//! its extras (a chimney with a hearth, a porch, a lean-to shed, a wing, flower boxes, a
//! lamp, a woodpile, moss in old walls) each come by their own chance, leaning on the
//! settlement's levers; its materials come from the few the settlement keeps of its
//! style's. Windows fall in a regular rhythm in orderly places and anywhere in others.

use voxmar_engine::block::BlockId;
use voxmar_engine::glam::IVec2;

use super::canvas::{Canvas, Frame, Palette, Put, unit};
use super::def::{Extra, Roof};
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
}

impl Kind {
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
    pub crops: Vec<[BlockId; 4]>,
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
    pub flowers: Palette,
    pub bushes: Palette,
    pub canopies: Palette,
    pub compost: Palette,
    pub boulder: Palette,
    pub fruit: Palette,
    pub dirt: BlockId,
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
    pub seed: u64,
}

impl Spec {
    pub fn has(&self, extra: Extra) -> bool {
        self.extras.contains(&extra)
    }

    /// The building's own box, everything it puts down included: overhangs, porch,
    /// shed, wing, chimney, the lamp and step at its door.
    pub fn envelope(&self) -> (IVec2, IVec2) {
        match self.kind {
            Kind::Well | Kind::Shrine | Kind::Stall => (IVec2::ZERO, IVec2::new(self.w - 1, self.d - 1)),
            Kind::Field | Kind::Pen | Kind::Orchard | Kind::Woodyard => (IVec2::ZERO, IVec2::new(self.w - 1, self.d - 1)),
            Kind::Smithy => (IVec2::new(-1, -2), IVec2::new(self.w, self.d)),
            _ => {
                let front = if self.has(Extra::Porch) || self.kind == Kind::Trader { -4 } else { -2 };
                let right = self.w + if self.shed > 0 { 3 } else { 0 };
                let back = self.d + self.wing.map_or(0, |(_, wd)| wd);
                let left = if self.has(Extra::Chimney) { -2 } else { -1 };
                (IVec2::new(left, front), IVec2::new(right, back))
            }
        }
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
    }
}

/// Puts blocks in a building's own coordinates.
struct Pen<'a> {
    frame: &'a Frame,
    canvas: &'a mut Canvas,
    seed: u64,
}

impl Pen<'_> {
    fn set(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
        self.canvas.set(self.frame.at(x, y, z), block);
    }

    fn put(&mut self, x: i32, y: i32, z: i32, put: Put) {
        self.canvas.put(self.frame.at(x, y, z), put);
    }

    fn clear(&mut self, x: i32, y: i32, z: i32) {
        self.put(x, y, z, Put::Clear);
    }

    fn pal(&mut self, x: i32, y: i32, z: i32, palette: &Palette) {
        let pos = self.frame.at(x, y, z);
        if let Some(block) = palette.at(self.seed, pos) {
            self.canvas.set(pos, block);
        }
    }

    /// On the ground of the column under (x, z), `up` over its top.
    fn ground(&mut self, x: i32, up: i32, z: i32, put: Put) {
        let p = self.frame.at(x, 0, z);
        self.canvas.on_ground(p.x, p.z, up, put);
    }

    fn roll(&self, x: i32, y: i32, z: i32, salt: u64) -> f32 {
        let p = self.frame.at(x, y, z);
        unit(hash3(self.seed ^ salt, p.x, p.y, p.z))
    }
}

fn house(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let (w, d) = (spec.w, spec.d);
    let lift = spec.stilts;
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let mut rng = Rng::new(spec.seed, 0x40e5, 0, 0);
    let top = spec.top() + lift;
    let moss = spec.has(Extra::Moss);
    // The wall block at a place: old walls take moss and cracks here and there.
    let wall_at = |pen: &Pen, x: i32, y: i32, z: i32| -> BlockId {
        if moss && pen.roll(x, y, z, 0x3055) < 0.22 {
            mats.weathered.at(pen.seed, pen.frame.at(x, y, z)).unwrap_or(mats.wall)
        } else {
            mats.wall
        }
    };

    // Foundation and floor; stilts lift the floor on posts.
    for z in 0..d {
        for x in 0..w {
            let edge = x == 0 || z == 0 || x == w - 1 || z == d - 1;
            if lift > 0 {
                pen.set(x, lift, z, if edge { mats.frame } else { mats.floor });
                for y in 1..lift {
                    pen.clear(x, y, z);
                }
                let post = (x == 0 || x == w - 1 || x == w / 2) && (z == 0 || z == d - 1 || z == d / 2);
                if post {
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
    let plinth = mats.wall != mats.frame && rng.chance(0.35);
    for storey in 0..spec.storeys {
        let base = lift + storey * (spec.wall + 1);
        for y in base + 1..=base + spec.wall {
            for z in 0..d {
                for x in 0..w {
                    let edge = x == 0 || z == 0 || x == w - 1 || z == d - 1;
                    if !edge {
                        pen.clear(x, y, z);
                        continue;
                    }
                    let corner = (x == 0 || x == w - 1) && (z == 0 || z == d - 1);
                    let block = if corner {
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
                    let edge = x == 0 || z == 0 || x == w - 1 || z == d - 1;
                    pen.set(x, y, z, if edge { mats.frame } else { mats.floor });
                }
            }
            // A hatch where a ladder will go.
            pen.clear(w - 2, y, d - 2);
        }
        windows(spec, &mut pen, mats, base, storey, &mut rng);
    }

    // The door, a lintel over it, and a step or stair to it.
    let door = spec.door.clamp(1, w - 2);
    let wide = spec.kind == Kind::Inn && door + 1 < w - 1;
    for dx in 0..=wide as i32 {
        pen.clear(door + dx, lift + 1, 0);
        pen.clear(door + dx, lift + 2, 0);
        if spec.wall >= 3 {
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

    roof(spec, &mut pen, mats, top, 0, 0, w, d, spec.ridge_x, mats.roof);

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
        roof(spec, &mut pen, mats, lift + wall, x0, d, ww, wd, false, mats.roof);
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
                    if edge && !(z == z0 + spec.shed / 2 && x == w + 1 && y <= lift + 2 && rng.chance(0.0)) {
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

    // Chimney and hearth on the left.
    let mut lit = false;
    if spec.has(Extra::Chimney) {
        let z = d / 2;
        let peak = top + (w.max(d) + 3) / 2 + 2;
        for y in lift..=peak {
            pen.pal(-1, y, z, &mats.foundation);
        }
        pen.put(-1, lift - 1, z, Put::Down(mats.foundation.0.first().map_or(mats.wall, |b| b.0)));
        pen.set(0, lift + 1, z, mats.foundation.0.first().map_or(mats.wall, |b| b.0));
        pen.set(1, lift + 1, z, fixed.campfire);
        lit = true;
    }

    // Inside: a table, a light, sometimes a planter.
    if d >= 5 && w >= 4 {
        let (tx, tz) = if rng.chance(0.5) { (w - 2, d - 2) } else { (w - 2, 1) };
        let (tx, tz) = if tx == door && tz == 1 { (1, d - 2) } else { (tx, tz) };
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
            pen.set(1, lift + 1, d - 2, planter);
        }
    }
    if !lit {
        pen.set(1, lift + 1, 1, fixed.torch);
    }
    if spec.kind == Kind::Inn {
        // Tables down the room.
        for z in (2..d - 2).step_by(3) {
            if w >= 7 {
                pen.set(w / 2, lift + 1, z, fixed.table);
            }
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
    if spec.kind == Kind::Trader {
        let canopy = fixed.canopies.pick(unit(hash3(spec.seed, 7, 7, 7))).unwrap_or(mats.trim);
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
    if spec.has(Extra::Lamp) || spec.kind == Kind::Inn {
        let sides: &[i32] = if spec.kind == Kind::Inn { &[-1, 2] } else { &[if rng.chance(0.5) { -1 } else { 1 }] };
        for &side in sides {
            let x = (door + side).clamp(-1, w);
            let x = if x == door { door - 1 } else { x };
            let z = if spec.has(Extra::Porch) { -4 } else { -2 };
            lamp(&mut pen, x, z, lift, mats.frame, fixed.torch);
        }
    }
    if spec.has(Extra::FlowerBoxes) && lift == 0 {
        for x in 1..w - 1 {
            if (x - door).abs() >= 2 && pen.roll(x, 1, -1, 0xf10) < 0.75 {
                pen.ground(x, 1, -1, Put::Soft(mats.trim));
                if let Some(flower) = fixed.flowers.at(spec.seed ^ 0xf1, frame.at(x, 2, -1)) {
                    pen.ground(x, 2, -1, Put::Soft(flower));
                }
            }
        }
    }
    if spec.has(Extra::Woodpile) && lift == 0 {
        // Against the left wall, logs stacked two high.
        let z0 = 1.max(d / 2 - 1);
        for z in z0..(z0 + 3).min(d - 1) {
            if spec.has(Extra::Chimney) && z == d / 2 {
                continue;
            }
            pen.ground(-1, 1, z, Put::Soft(mats.log));
            if pen.roll(-1, 2, z, 0x3d) < 0.6 {
                pen.ground(-1, 2, z, Put::Soft(mats.log));
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

/// Windows on every wall of one storey.
fn windows(spec: &Spec, pen: &mut Pen, mats: &Mats, base: i32, storey: i32, rng: &mut Rng) {
    let (w, d) = (spec.w, spec.d);
    let y = base + if spec.wall >= 3 { 2 } else { 1 };
    let chance = spec.window_chance;
    let phase = rng.below(2);
    let place = |pen: &mut Pen, x: i32, z: i32, along: i32, rng: &mut Rng| {
        let wanted = if spec.regular { (along + phase) % 2 == 0 && rng.chance(chance + 0.3) } else { rng.chance(chance * 0.7) };
        if !wanted {
            return;
        }
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
    for z in 1..d - 1 {
        if !(spec.has(Extra::Chimney) && z == d / 2) {
            place(pen, 0, z, z, rng);
        }
        if !(spec.shed > 0 && storey == 0) {
            place(pen, w - 1, z, z, rng);
        }
    }
}

/// A roof over the walls of the box from (x0, z0), `w` by `d`, whose walls top out at
/// `top`.
#[allow(clippy::too_many_arguments)]
fn roof(spec: &Spec, pen: &mut Pen, mats: &Mats, top: i32, x0: i32, z0: i32, w: i32, d: i32, ridge_x: bool, block: BlockId) {
    let pitch = spec.pitch.max(1);
    match spec.roof {
        Roof::Flat => {
            for z in z0..z0 + d {
                for x in x0..x0 + w {
                    pen.set(x, top + 1, z, block);
                    let edge = x == x0 || z == z0 || x == x0 + w - 1 || z == z0 + d - 1;
                    if edge && pen.roll(x, top + 2, z, 0x9a2) < 0.85 {
                        pen.set(x, top + 2, z, mats.trim);
                    }
                }
            }
        }
        Roof::Shed => {
            // Highest over the front, falling to the back.
            let height = |z: i32| top + 1 + (z0 + d - z) / (pitch + 1);
            for z in z0 - 1..=z0 + d {
                let y = height(z);
                for x in x0 - 1..=x0 + w {
                    pen.set(x, y, z, block);
                }
                // The side walls rise to meet it, the front wall too.
                for yy in top + 1..y {
                    if (z0..z0 + d).contains(&z) {
                        for x in [x0, x0 + w - 1] {
                            pen.set(x, yy, z, mats.wall);
                        }
                        for x in x0 + 1..x0 + w - 1 {
                            if z == z0 {
                                pen.set(x, yy, z, mats.wall);
                            } else {
                                pen.clear(x, yy, z);
                            }
                        }
                    }
                }
            }
        }
        Roof::Gable => {
            // Across the span, inwards from the eaves; along the ridge, one over at each end.
            let (span, run) = if ridge_x { (d, w) } else { (w, d) };
            let at = |s: i32, r: i32| if ridge_x { (x0 + r, z0 + s) } else { (x0 + s, z0 + r) };
            let (mut lo, mut hi, mut y) = (-1, span, top + 1);
            while lo <= hi {
                for s in (lo..lo + pitch).chain(hi + 1 - pitch..=hi) {
                    if s < lo || s > hi {
                        continue;
                    }
                    for r in -1..=run {
                        let (x, z) = at(s, r);
                        pen.set(x, y, z, block);
                    }
                }
                // The gable ends between, and the loft within.
                for s in lo + pitch..=hi - pitch {
                    for r in 0..run {
                        let (x, z) = at(s, r);
                        if r == 0 || r == run - 1 {
                            pen.set(x, y, z, if s == span / 2 && (r == 0 || r == run - 1) && y == top + 2 { mats.window.unwrap_or(mats.trim) } else { mats.wall });
                        } else {
                            pen.clear(x, y, z);
                        }
                    }
                }
                lo += pitch;
                hi -= pitch;
                y += 1;
            }
        }
        Roof::Hip => {
            let (mut lx, mut hx, mut lz, mut hz, mut y) = (x0 - 1, x0 + w, z0 - 1, z0 + d, top + 1);
            while lx <= hx && lz <= hz {
                for z in lz..=hz {
                    for x in lx..=hx {
                        let ring = x < lx + pitch || x > hx - pitch || z < lz + pitch || z > hz - pitch;
                        if ring {
                            pen.set(x, y, z, block);
                        } else {
                            pen.clear(x, y, z);
                        }
                    }
                }
                lx += pitch;
                hx -= pitch;
                lz += pitch;
                hz -= pitch;
                y += 1;
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
    roof(spec, &mut pen, mats, spec.wall, 0, 0, w, d, spec.ridge_x, mats.roof);
    // The forge in a back corner.
    let stone = mats.foundation.0.first().map_or(mats.wall, |b| b.0);
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

/// A wayside shrine: a stepped plinth, a pillar and a light.
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
    pen.ground(cx, 1, cz, Put::Set(mats.frame));
    pen.ground(cx, 2, cz, Put::Set(mats.frame));
    pen.ground(cx, 3, cz, Put::Set(fixed.torch));
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
    let crop = if mats.crops.is_empty() { None } else { Some(mats.crops[rng.below(mats.crops.len() as u32) as usize]) };
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
                    let u = (growth * 3.0 + rng.unit() * 1.4 - 0.4).clamp(0.0, 3.0) as usize;
                    pen.set(x, 1, z, stages[u.min(3)]);
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
fn orchard(spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let (w, d) = (spec.w, spec.d);
    let leaves = fixed.fruit.pick(unit(hash3(spec.seed, 3, 1, 4))).unwrap_or(mats.trim);
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
