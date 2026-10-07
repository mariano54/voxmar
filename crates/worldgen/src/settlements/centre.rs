//! Centrepieces: the one great thing a village is known by (a great tree on the green, a
//! stone circle, a windmill), drawn in their own coordinates like buildings, the ground at
//! y = 0 graded flat under them. Each is made of its parts (`parts` in settlements.ron),
//! any left out coming from the settlement's style.

use voxmar_engine::block::BlockId;

use super::canvas::{Canvas, Frame, Put, unit};
use super::def::Centre;
use super::house::{Fixed, Mats, Pen, Spec};
use crate::rng::{Rng, hash3};

/// The blocks a centrepiece is made of.
pub struct Parts {
    pub log: BlockId,
    pub leaves: Option<BlockId>,
    pub stone: BlockId,
    /// Stone grown old: moss, cracks.
    pub worn: Option<BlockId>,
    pub water: BlockId,
    pub cloth: BlockId,
    pub light: BlockId,
    pub bell: BlockId,
}

impl Parts {
    /// Its parts by name, from `named`, the rest from the style and the furnishings.
    pub fn new(named: &dyn Fn(&str) -> Option<BlockId>, mats: &Mats, fixed: &Fixed, seed: u64) -> Self {
        let cloth = fixed.cloth.pick(unit(hash3(seed, 0xc1, 0, 0))).or(fixed.canopies.pick(unit(hash3(seed, 0xc1, 0, 0)))).unwrap_or(mats.trim);
        Parts {
            log: named("log").unwrap_or(mats.log),
            leaves: named("leaves").or(mats.fruit.pick(unit(hash3(seed, 0x1ea, 0, 0)))),
            stone: named("stone").unwrap_or(mats.stone()),
            worn: named("worn"),
            water: named("water").unwrap_or(fixed.water),
            cloth: named("cloth").unwrap_or(cloth),
            light: named("light").unwrap_or(fixed.torch),
            bell: named("bell").or(fixed.bell).unwrap_or(fixed.anvil),
        }
    }
}

pub fn draw(centre: Centre, spec: &Spec, frame: &Frame, mats: &Mats, fixed: &Fixed, parts: &Parts, canvas: &mut Canvas) {
    let mut pen = Pen { frame, canvas, seed: spec.seed };
    let mut rng = Rng::new(spec.seed, 0xce47e, 0, 0);
    let (w, d) = (spec.w, spec.d);
    let (cx, cz) = (w / 2, d / 2);
    // Old stone takes moss and cracks.
    let worn = |pen: &Pen, x: i32, y: i32, z: i32, chance: f32| -> BlockId {
        if pen.roll(x, y, z, 0x3055) < chance {
            parts.worn.or(mats.weathered.at(pen.seed, pen.frame.at(x, y, z))).unwrap_or(parts.stone)
        } else {
            parts.stone
        }
    };
    let flowers = |pen: &mut Pen, r: f32, chance: f32| {
        for z in 0..d {
            for x in 0..w {
                let e = (((x - cx).pow(2) + (z - cz).pow(2)) as f32).sqrt();
                if (e - r).abs() < 0.7
                    && pen.roll(x, 1, z, 0xf1) < chance
                    && let Some(f) = fixed.flowers.at(pen.seed ^ 0xf2, pen.frame.at(x, 1, z))
                {
                    pen.put(x, 1, z, Put::Soft(f));
                }
            }
        }
    };
    match centre {
        Centre::GreatTree => {
            // A trunk two blocks thick on flared roots, four boughs and a broad crown.
            let height = 10 + rng.below(3);
            let (tx, tz) = (cx - 1, cz - 1);
            for y in 1..=height {
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    pen.set(tx + dx, y, tz + dz, parts.log);
                }
            }
            for (x, z) in [(tx - 1, tz), (tx - 1, tz + 1), (tx + 2, tz), (tx + 2, tz + 1), (tx, tz - 1), (tx + 1, tz - 1), (tx, tz + 2), (tx + 1, tz + 2)] {
                if pen.roll(x, 1, z, 0x200) < 0.6 {
                    pen.set(x, 1, z, parts.log);
                    if pen.roll(x, 2, z, 0x201) < 0.35 {
                        pen.set(x, 2, z, parts.log);
                    }
                }
            }
            for (dx, dz, x0, z0) in [(-1, -1, tx, tz), (1, -1, tx + 1, tz), (-1, 1, tx, tz + 1), (1, 1, tx + 1, tz + 1)] {
                let from = height - 4 + rng.below(2);
                for k in 1..=3 {
                    pen.set(x0 + dx * k, from + k, z0 + dz * k, parts.log);
                }
            }
            if let Some(leaves) = parts.leaves {
                let (r, ry) = (5.6, 3.4);
                let (mx, mz) = (tx as f32 + 0.5, tz as f32 + 0.5);
                for y in height - 3..=height + 4 {
                    for z in 0..d {
                        for x in 0..w {
                            let (dx, dy, dz) = ((x as f32 - mx) / r, (y - height) as f32 / ry, (z as f32 - mz) / r);
                            let e = dx * dx + dy * dy + dz * dz;
                            if e <= 1.0 && (e < 0.65 || pen.roll(x, y, z, 0x1e) < 0.65) {
                                pen.put(x, y, z, Put::Soft(leaves));
                            }
                        }
                    }
                }
            }
            flowers(&mut pen, 3.5, 0.3);
        }
        Centre::StoneCircle => {
            let n = 7 + rng.below(3);
            let start = rng.unit() * std::f32::consts::TAU;
            let fallen = rng.below(n as u32);
            for k in 0..n {
                let a = start + k as f32 * std::f32::consts::TAU / n as f32;
                let (x, z) = (cx + (a.cos() * 4.4).round() as i32, cz + (a.sin() * 4.4).round() as i32);
                if k == fallen {
                    // Lying along the ring.
                    let (tx, tz) = (-a.sin(), a.cos());
                    for s in 0..3 {
                        let (px, pz) = (x + (tx * s as f32).round() as i32, z + (tz * s as f32).round() as i32);
                        if (0..w).contains(&px) && (0..d).contains(&pz) {
                            pen.set(px, 1, pz, worn(&pen, px, 1, pz, 0.5));
                        }
                    }
                    continue;
                }
                let tall = 2 + rng.below(3);
                for y in 1..=tall {
                    pen.set(x, y, z, worn(&pen, x, y, z, 0.35));
                }
                // Some stones are broad, a block thicker out from the middle.
                if rng.chance(0.35) {
                    let (ox, oz) = (x + (a.cos() * 1.4).round() as i32, z + (a.sin() * 1.4).round() as i32);
                    if (0..w).contains(&ox) && (0..d).contains(&oz) {
                        for y in 1..tall {
                            pen.set(ox, y, oz, worn(&pen, ox, y, oz, 0.35));
                        }
                    }
                }
            }
            pen.set(cx, 1, cz, parts.stone);
            pen.set(cx + 1, 1, cz, parts.stone);
        }
        Centre::WatchtowerRuin => {
            let height = 8 + rng.below(4);
            // The fallen side, where the walls are lowest.
            let low = rng.below(4);
            for z in 1..=5 {
                for x in 1..=5 {
                    let wall = x == 1 || x == 5 || z == 1 || z == 5;
                    if !wall {
                        for y in 1..=height {
                            pen.clear(x, y, z);
                        }
                        continue;
                    }
                    let side = match (x, z) {
                        (1, _) => 0,
                        (5, _) => 1,
                        (_, 1) => 2,
                        _ => 3,
                    };
                    let broken = (pen.roll(x, 0, z, 0x70) * 4.0) as i32 + if side == low { 4 } else { 0 };
                    let top = (height - broken).max(1);
                    for y in 1..=top {
                        let slit = (x == 3 || z == 3) && y % 4 == 3;
                        if slit {
                            pen.clear(x, y, z);
                        } else {
                            pen.set(x, y, z, worn(&pen, x, y, z, 0.4));
                        }
                    }
                }
            }
            pen.clear(3, 1, 1);
            pen.clear(3, 2, 1);
            // Rubble fallen round it.
            for z in 0..d {
                for x in 0..w {
                    let outside = !(1..=5).contains(&x) || !(1..=5).contains(&z);
                    if outside
                        && pen.roll(x, 1, z, 0x7b) < 0.22
                        && let Some(b) = fixed.boulder.at(pen.seed, pen.frame.at(x, 1, z))
                    {
                        pen.put(x, 1, z, Put::Soft(b));
                    }
                }
            }
        }
        Centre::Windmill => {
            // A stone foot, a body of the style's walls, a cap and four sails.
            let height = 9;
            for z in 1..=5 {
                for x in 3..=7 {
                    let wall = x == 3 || x == 7 || z == 1 || z == 5;
                    for y in 1..=height {
                        if !wall {
                            pen.clear(x, y, z);
                        } else if y <= 3 {
                            pen.set(x, y, z, worn(&pen, x, y, z, 0.15));
                        } else {
                            pen.set(x, y, z, if (x == 3 || x == 7) && (z == 1 || z == 5) { mats.frame } else { mats.wall });
                        }
                    }
                }
            }
            for z in 0..=6 {
                for x in 2..=8 {
                    let up = (3 - (x - 5i32).abs()).max(0) / 2;
                    pen.set(x, height + 1 + up, z, mats.roof);
                }
            }
            pen.clear(5, 1, 1);
            pen.clear(5, 2, 1);
            let hub = 7;
            pen.set(5, hub, 0, mats.frame);
            for k in 1..=4 {
                pen.set(5, hub + k, 0, mats.frame);
                pen.set(6, hub + k, 0, parts.cloth);
                pen.set(5, hub - k, 0, mats.frame);
                pen.set(4, hub - k, 0, parts.cloth);
                pen.set(5 + k, hub, 0, mats.frame);
                pen.set(5 + k, hub - 1, 0, parts.cloth);
                pen.set(5 - k, hub, 0, mats.frame);
                pen.set(5 - k, hub + 1, 0, parts.cloth);
            }
        }
        Centre::BellFrame => {
            for x in [0, w - 1] {
                for y in 1..=5 {
                    pen.set(x, y, 1, mats.frame);
                }
            }
            for x in 0..w {
                pen.set(x, 6, 1, mats.trim);
                for z in 0..d {
                    pen.set(x, 7, z, mats.roof);
                }
            }
            pen.set(cx, 5, 1, mats.frame);
            pen.set(cx, 4, 1, parts.bell);
        }
        Centre::FallenStatue => {
            // The plinth with the broken legs still on it; the rest lying beside, face up.
            for z in 1..=3 {
                for x in 0..=2 {
                    pen.set(x, 1, z, worn(&pen, x, 1, z, 0.3));
                    pen.set(x, 2, z, worn(&pen, x, 2, z, 0.2));
                }
            }
            for z in [1, 3] {
                pen.set(1, 3, z, parts.stone);
                if pen.roll(1, 4, z, 0xfa) < 0.6 {
                    pen.set(1, 4, z, worn(&pen, 1, 4, z, 0.4));
                }
            }
            let body = [(4, 1), (5, 1), (4, 3), (5, 3), (6, 1), (6, 2), (6, 3), (7, 1), (7, 2), (7, 3), (8, 1), (8, 2), (8, 3), (7, 0), (7, 4), (6, 0), (6, 4), (9, 2), (10, 2)];
            for (x, z) in body {
                pen.set(x, 1, z, worn(&pen, x, 1, z, 0.45));
            }
            for (x, z) in [(7, 2), (8, 2), (9, 2), (10, 2)] {
                pen.set(x, 2, z, worn(&pen, x, 2, z, 0.45));
            }
            for (x, z) in [(3, 0), (5, 4), (2, 4)] {
                if let Some(b) = fixed.boulder.at(pen.seed, pen.frame.at(x, 1, z)) {
                    pen.put(x, 1, z, Put::Soft(b));
                }
            }
        }
        Centre::Maypole => {
            for y in 1..=10 {
                pen.set(cx, y, cz, mats.log);
            }
            if let Some(leaves) = parts.leaves {
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    pen.set(cx + dx, 9, cz + dz, leaves);
                }
            }
            for (x, z) in [(0, 0), (w - 1, 0), (0, d - 1), (w - 1, d - 1)] {
                pen.set(x, 1, z, parts.cloth);
            }
            pen.set(cx, 11, cz, parts.cloth);
            flowers(&mut pen, 1.5, 0.5);
        }
        Centre::MarketCross => {
            for z in 0..d {
                for x in 0..w {
                    let ring = (x - cx).abs().max((z - cz).abs());
                    for y in 1..=3 - ring.min(2) {
                        if ring <= 2 {
                            pen.set(x, y, z, worn(&pen, x, y, z, 0.25));
                        }
                    }
                }
            }
            for y in 2..=7 {
                pen.set(cx, y, cz, parts.stone);
            }
            pen.set(cx - 1, 6, cz, parts.stone);
            pen.set(cx + 1, 6, cz, parts.stone);
        }
        Centre::Fountain => {
            for z in 0..d {
                for x in 0..w {
                    let rim = x == 0 || z == 0 || x == w - 1 || z == d - 1;
                    if rim {
                        pen.set(x, 0, z, parts.stone);
                        pen.set(x, 1, z, worn(&pen, x, 1, z, 0.2));
                    } else {
                        pen.set(x, 0, z, parts.water);
                        pen.set(x, -1, z, parts.water);
                        pen.set(x, -2, z, parts.stone);
                        pen.clear(x, 1, z);
                    }
                }
            }
            for y in -1..=2 {
                pen.set(cx, y, cz, parts.stone);
            }
            pen.set(cx, 3, cz, parts.light);
        }
        Centre::Beacon => {
            for z in 1..=3 {
                for x in 1..=3 {
                    for y in 1..=5 {
                        pen.set(x, y, z, worn(&pen, x, y, z, 0.2));
                    }
                }
            }
            for (x, z) in [(1, 1), (3, 1), (1, 3), (3, 3)] {
                pen.set(x, 6, z, parts.stone);
            }
            pen.set(cx, 6, cz, fixed.campfire);
            // Steps up one side.
            for k in 1..=4 {
                pen.set(0, k, 5 - k, parts.stone);
                pen.put(0, k - 1, 5 - k, Put::Down(parts.stone));
            }
        }
        Centre::StandingStone => {
            let height = 4 + rng.below(3);
            for y in 1..=height {
                pen.set(cx, y, cz, worn(&pen, cx, y, cz, 0.3));
            }
            pen.set(cx, 1, cz + 1, worn(&pen, cx, 1, cz + 1, 0.3));
            pen.set(cx, 2, cz + 1, worn(&pen, cx, 2, cz + 1, 0.3));
            flowers(&mut pen, 2.0, 0.4);
        }
        Centre::DuckPond => {
            let (rx, rz) = (w as f32 / 2.0 - 0.8, d as f32 / 2.0 - 0.8);
            for z in 0..d {
                for x in 0..w {
                    let (dx, dz) = ((x - cx) as f32 / rx, (z - cz) as f32 / rz);
                    let e = dx * dx + dz * dz + (pen.roll(x, 0, z, 0xd0) - 0.5) * 0.3;
                    if e <= 1.0 {
                        pen.set(x, 0, z, parts.water);
                        if e < 0.45 {
                            pen.set(x, -1, z, parts.water);
                            pen.put(x, -2, z, Put::Set(fixed.dirt));
                        } else {
                            pen.set(x, -1, z, fixed.dirt);
                        }
                        pen.clear(x, 1, z);
                    } else if e <= 1.5
                        && pen.roll(x, 1, z, 0xd1) < 0.3
                        && let Some(b) = fixed.bushes.at(pen.seed, pen.frame.at(x, 1, z)).or(fixed.flowers.at(pen.seed, pen.frame.at(x, 1, z)))
                    {
                        pen.put(x, 1, z, Put::Soft(b));
                    }
                }
            }
        }
    }
}
