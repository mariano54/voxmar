//! From a routed trail to the ground: a smooth centreline, the height the trail walks at,
//! and per-column stamps that cut the trail into slopes and raise it on low banks.
//!
//! Following RoadWeaver (MIT): the polyline is relaxed and drawn as a Catmull-Rom curve
//! sampled about every block; its height is the ground averaged along the trail, held
//! within a few blocks of the ground and limited in how fast it rises by a forward and a
//! backward pass; water is crossed on a deck one block above it, or on stepping stones
//! where it is shallow, with the approaches ramped. Each column near the trail then
//! blends its ground towards the trail's height, fully under the tread and fading over a
//! bank on either side, taking the strongest trail where two meet rather than adding
//! them.

use voxmar_engine::glam::{IVec2, Vec2};

use super::route::FORD_DEPTH;
use super::{Land, TrailMark};
use crate::water::cache::Weigh;

/// Half the width of the tread, in blocks.
pub const HALF: f32 = 1.5;
/// Width of the bank over which the ground blends back to its own height.
pub const BANK: f32 = 3.0;
/// Furthest from the centreline a trail changes anything.
pub const REACH: f32 = HALF + BANK;
/// Most a trail is raised above or cut below the ground.
const FILL: f32 = 5.0;
const CUT: f32 = 6.0;
/// Most a trail rises per block.
const RISE: f32 = 0.6;
/// Centres either side averaged for the trail's height.
const AVERAGE: usize = 3;
/// Centres over which the ground ramps up to a bridge deck.
const RAMP: usize = 4;
/// Bridges at most this long (in centres, about blocks) are a few logs across a creek.
const LOG_BRIDGE: usize = 7;
/// Centres between the piers of a longer bridge.
const PIER_SPACING: usize = 6;
/// Blocks of trail between cairns, where cairns are built.
const CAIRN_SPACING: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crossing {
    Ground,
    /// Shallow water, crossed on stepping stones.
    Ford,
    /// Deeper water, crossed on a deck at this height: logs over a creek, planks over a
    /// river.
    Bridge { deck: i32, log: bool },
}

/// A point on a trail's centreline, about a block from the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Centre {
    pub at: Vec2,
    /// The height the tread lies at (the top of its ground block).
    pub y: f32,
    pub crossing: Crossing,
}

/// One continuous stretch of trail.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub centres: Vec<Centre>,
    pub min: Vec2,
    pub max: Vec2,
}

/// The drawn trails of a region.
pub struct Paths(pub Vec<Path>);

impl Weigh for Paths {
    fn bytes(&self) -> usize {
        self.0.iter().map(|p| std::mem::size_of::<Path>() + p.centres.len() * std::mem::size_of::<Centre>()).sum()
    }
}

/// The trail along `points` (blocks), with its height from `land`.
pub fn path(points: &[Vec2], land: &dyn Land) -> Path {
    let curve = curve(&relax(points));
    let mut centres: Vec<Centre> = curve
        .iter()
        .map(|&at| {
            let ground = land.sample(at.x.floor() as i32, at.y.floor() as i32);
            let depth = ground.water - ground.height;
            let crossing = match depth {
                d if d <= 0 => Crossing::Ground,
                d if d <= FORD_DEPTH => Crossing::Ford,
                _ => Crossing::Bridge { deck: ground.water + 1, log: false },
            };
            // On a ford the stones are flush with the water.
            let y = match crossing {
                Crossing::Ground => ground.height,
                Crossing::Ford => ground.water,
                Crossing::Bridge { deck, .. } => deck,
            } as f32;
            Centre { at, y, crossing }
        })
        .collect();
    // A crossing that is deep anywhere is bridged all the way, level, a block above its
    // highest water; short ones on logs. Shallow crossings stay fords.
    let mut i = 0;
    while i < centres.len() {
        let span = centres[i..].iter().take_while(|c| c.crossing != Crossing::Ground).count();
        let run = &mut centres[i..i + span];
        if run.iter().any(|c| matches!(c.crossing, Crossing::Bridge { .. })) {
            let deck = run
                .iter()
                .map(|c| match c.crossing {
                    Crossing::Bridge { deck, .. } => deck,
                    _ => c.y as i32 + 1,
                })
                .max()
                .expect("a bridge");
            for c in run.iter_mut() {
                c.crossing = Crossing::Bridge { deck, log: span <= LOG_BRIDGE };
                c.y = deck as f32;
            }
        }
        i += span.max(1);
    }
    heights(&mut centres);
    let min = centres.iter().fold(Vec2::splat(f32::MAX), |m, c| m.min(c.at)) - REACH;
    let max = centres.iter().fold(Vec2::splat(f32::MIN), |m, c| m.max(c.at)) + REACH;
    Path { centres, min, max }
}

/// Eases sharp corners: each inner point moves towards its neighbours twice.
fn relax(points: &[Vec2]) -> Vec<Vec2> {
    let mut points = points.to_vec();
    for _ in 0..2 {
        let before = points.clone();
        for i in 1..points.len().saturating_sub(1) {
            points[i] = (before[i - 1] + before[i] * 2.0 + before[i + 1]) / 4.0;
        }
    }
    points
}

/// A Catmull-Rom curve through `points`, sampled about every block.
fn curve(points: &[Vec2]) -> Vec<Vec2> {
    let mut out = vec![points[0]];
    for i in 0..points.len() - 1 {
        let (p0, p1, p2) = (points[i.saturating_sub(1)], points[i], points[i + 1]);
        let p3 = points[(i + 2).min(points.len() - 1)];
        let steps = (p1.distance(p2).ceil() as usize).max(1);
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 + p2 * -3.0 + p3) * t3));
        }
    }
    out
}

/// Sets the height the trail walks at: the ground averaged along it and kept near the
/// ground, with water crossings fixed, ramps up to decks, and the rise per block limited
/// both ways.
fn heights(centres: &mut [Centre]) {
    let n = centres.len();
    let ground: Vec<f32> = centres.iter().map(|c| c.y).collect();
    let fixed = |c: &Centre| c.crossing != Crossing::Ground;
    for i in 0..n {
        if fixed(&centres[i]) {
            continue;
        }
        let window = &ground[i.saturating_sub(AVERAGE)..(i + AVERAGE + 1).min(n)];
        let average = window.iter().sum::<f32>() / window.len() as f32;
        centres[i].y = average.clamp(ground[i] - CUT, ground[i] + FILL);
    }
    // Approaches climb to the deck.
    for i in 0..n {
        if let Crossing::Bridge { deck, .. } = centres[i].crossing {
            for k in 1..=RAMP {
                let w = 1.0 - k as f32 / (RAMP + 1) as f32;
                for j in [i.wrapping_sub(k), i + k] {
                    if j < n && !fixed(&centres[j]) {
                        centres[j].y = centres[j].y.max(centres[j].y + (deck as f32 - centres[j].y) * w);
                    }
                }
            }
        }
    }
    for i in 1..n {
        if !fixed(&centres[i]) {
            let before = centres[i - 1].y;
            centres[i].y = centres[i].y.clamp(before - RISE, before + RISE);
        }
    }
    for i in (0..n.saturating_sub(1)).rev() {
        if !fixed(&centres[i]) {
            let after = centres[i + 1].y;
            centres[i].y = centres[i].y.clamp(after - RISE, after + RISE);
        }
    }
}

/// What a trail does to one column.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stamp {
    /// The trail's height here, and how fully the ground takes it (0..1).
    pub target: f32,
    pub weight: f32,
    /// How much of the tread lies here, 0..1.
    pub tread: f32,
    /// A bridge deck's height, over water, and whether it is of logs.
    pub deck: Option<i32>,
    pub log: bool,
    /// A pier holding the deck up, from the bed.
    pub pier: bool,
    /// A stepping stone.
    pub stone: bool,
    /// No tree may stand here: on the trail, or in the clearing at a summit or viewpoint.
    pub clear: bool,
}

impl Stamp {
    pub fn mark(&self) -> Option<TrailMark> {
        (self.weight > 0.0 || self.tread > 0.0 || self.clear).then_some(TrailMark {
            tread: self.tread,
            deck: self.deck,
            log: self.log,
            pier: self.pier,
            stone: self.stone,
            clear: self.clear,
        })
    }
}

/// Stamps over a `size`×`size` square of columns from `min`, from the trails `paths`.
pub fn stamps(min: IVec2, size: i32, paths: &[&Path], clearings: &[IVec2]) -> Option<Box<[Stamp]>> {
    let mut stamps = vec![Stamp::default(); (size * size) as usize];
    let mut any = false;
    for &centre in clearings {
        for z in (centre.y - CLEARING).max(min.y)..=(centre.y + CLEARING).min(min.y + size - 1) {
            for x in (centre.x - CLEARING).max(min.x)..=(centre.x + CLEARING).min(min.x + size - 1) {
                if (IVec2::new(x, z) - centre).length_squared() <= CLEARING * CLEARING {
                    stamps[((z - min.y) * size + x - min.x) as usize].clear = true;
                    any = true;
                }
            }
        }
    }
    let (low, high) = (min.as_vec2(), (min + size).as_vec2());
    for path in paths {
        if path.max.cmplt(low).any() || path.min.cmpge(high).any() {
            continue;
        }
        for (i, w) in path.centres.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            let from = (a.at.min(b.at) - REACH).floor().as_ivec2().max(min);
            let to = (a.at.max(b.at) + REACH).ceil().as_ivec2().min(min + size - 1);
            for z in from.y..=to.y {
                for x in from.x..=to.x {
                    let p = Vec2::new(x as f32 + 0.5, z as f32 + 0.5);
                    let ab = b.at - a.at;
                    let t = ((p - a.at).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let d = p.distance(a.at + ab * t);
                    if d >= REACH {
                        continue;
                    }
                    let stamp = &mut stamps[((z - min.y) * size + x - min.x) as usize];
                    let weight = bank(d);
                    if weight > stamp.weight {
                        stamp.weight = weight;
                        stamp.target = a.y + (b.y - a.y) * t;
                    }
                    stamp.tread = stamp.tread.max((HALF + 0.5 - d).clamp(0.0, 1.0));
                    stamp.clear |= weight >= 0.5;
                    if d <= HALF {
                        let crossing = if t < 0.5 { a.crossing } else { b.crossing };
                        match crossing {
                            Crossing::Bridge { deck, log } => {
                                stamp.deck = Some(stamp.deck.map_or(deck, |d| d.max(deck)));
                                stamp.log |= log;
                                stamp.pier |= !log && i % PIER_SPACING == 0 && p.distance(a.at) <= 0.75;
                            }
                            Crossing::Ford => stamp.stone |= (x + z) % 2 == 0 && d <= HALF - 0.5,
                            Crossing::Ground => {}
                        }
                    }
                    any = true;
                }
            }
        }
    }
    any.then(|| stamps.into())
}

/// Radius of the clearing at a summit or viewpoint, in blocks.
const CLEARING: i32 = 5;

/// Where cairns go along a trail: every so often, a little to one side and then the
/// other, wherever the trail is on the ground. Whether one is built depends on the land
/// (they mark trails above the treeline).
pub fn cairns(path: &Path) -> Vec<IVec2> {
    let mut cairns = Vec::new();
    let mut walked = CAIRN_SPACING / 2.0;
    for w in path.centres.windows(2) {
        walked += w[0].at.distance(w[1].at);
        if walked < CAIRN_SPACING || w[0].crossing != Crossing::Ground {
            continue;
        }
        walked = 0.0;
        let along = (w[1].at - w[0].at).normalize_or(Vec2::X);
        let side = if cairns.len() % 2 == 0 { 1.0 } else { -1.0 };
        cairns.push((w[0].at + Vec2::new(-along.y, along.x) * side * (HALF + 1.1)).floor().as_ivec2());
    }
    cairns
}

/// How fully the ground takes the trail's height at `d` blocks from its centreline: all
/// of it under the tread, easing to none across the bank (a squared smootherstep).
fn bank(d: f32) -> f32 {
    let t = ((HALF + BANK - d) / BANK).clamp(0.0, 1.0);
    let s = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    s * s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geology::GeoSample;
    use crate::water::Body;

    struct Slope;

    impl Land for Slope {
        fn sample(&self, x: i32, _: i32) -> GeoSample {
            // Rising one block in four eastwards, with a creek at x = 40..42 and a river
            // two blocks deep at x = 80..86.
            let height = 140 + x / 4;
            let (height, water) = match x {
                40..=42 => (height - 1, height),
                80..=86 => (height - 3, height - 1),
                _ => (height, height),
            };
            GeoSample { height, mountain: 0.0, river: 0.0, water, body: Body::River, current: None, cavity: None, trail: None, form: Default::default() }
        }
    }

    #[test]
    fn walks_smoothly_and_crosses_water() {
        let points: Vec<Vec2> = (0..=16).map(|i| Vec2::new(i as f32 * 8.0, 0.0)).collect();
        let path = path(&points, &Slope);
        assert!(path.centres.len() >= 120);
        assert!(path.centres.windows(2).all(|w| (w[1].y - w[0].y).abs() <= RISE + 1e-4 || w.iter().any(|c| c.crossing != Crossing::Ground)));
        assert!(path.centres.iter().any(|c| c.crossing == Crossing::Ford));
        let deck = path.centres.iter().find_map(|c| if let Crossing::Bridge { deck, log } = c.crossing { Some((deck, log)) } else { None }).expect("a bridge");
        // Seven blocks of river: a level log bridge a block above its highest water.
        assert_eq!(deck, (140 + 86 / 4 - 1 + 1, true));
        let decks: Vec<_> = path.centres.iter().filter_map(|c| if let Crossing::Bridge { deck, .. } = c.crossing { Some(deck) } else { None }).collect();
        assert!(decks.iter().all(|&d| d == deck.0));
        // Stamps: full under the tread, nothing beyond the bank, decks over the river.
        let stamps = stamps(IVec2::new(0, -8), 128, &[&path], &[]).unwrap();
        let at = |x: i32, z: i32| stamps[((z + 8) * 128 + x) as usize];
        assert_eq!(at(20, 0).weight, 1.0);
        assert!(at(20, 0).tread > 0.9);
        assert_eq!(at(20, 7).weight, 0.0);
        assert!(at(83, 0).deck.is_some());
        assert!((40..=42).any(|x| at(x, 0).stone || at(x, -1).stone || at(x, 1).stone));
    }
}
