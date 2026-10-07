//! Planning one settlement: where in its square it stands, its character (levers), what
//! it is built of, its lanes, what stands along them, and the ground it grades.
//!
//! Everything here reads only the land with its rivers and lakes (no trails, which are
//! laid after settlements) and the climate, so a plan is a pure function of the seed and
//! its square.

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::glam::{IVec2, IVec3, Vec2};

use super::canvas::{Canvas, Facing, Frame, Palette, Put, unit};
use super::def::{CentreKind, Extra, Form, Lever, Levers, Prop, PropKind, Roof, SettlementsDef, Shape, Special, Spot, StyleDef, fill};
use super::house::{self, Fixed, Kind, Mats, Spec};
use super::centre;
use super::{Building, Plan, Raster};
use crate::geology::GeoSample;
use crate::rng::{Rng, hash3};
use crate::trails::Land;
use crate::{Overworld, SEA_LEVEL};

/// Blocks between samples when looking for a site.
const STEP: i32 = 16;
/// How far round a site must be dry, firm and gentle, in blocks.
const SITE_RADIUS: i32 = 48;

/// What the planner works with.
pub struct Ctx<'a> {
    pub world: &'a Overworld,
    pub land: &'a dyn Land,
    pub def: &'a SettlementsDef,
    pub resolved: &'a super::def::Resolved,
    pub fixed: &'a Fixed,
    pub seed: u64,
}

/// Picks from `items` by weight; `None` when every weight is nothing.
fn pick<T: Copy>(rng: &mut Rng, items: &[(T, f32)]) -> Option<T> {
    let total: f32 = items.iter().map(|(_, w)| w.max(0.0)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut t = rng.unit() * total;
    for &(item, w) in items {
        let w = w.max(0.0);
        if t < w {
            return Some(item);
        }
        t -= w;
    }
    items.last().map(|(i, _)| *i)
}

fn between(rng: &mut Rng, (lo, hi): (u32, u32)) -> i32 {
    rng.range(lo as i32, hi as i32 + 1)
}

/// Whether ground can take a building or a lane.
fn firm(g: &GeoSample) -> bool {
    !g.submerged() && g.river < 0.08 && g.crag.amplitude <= 0.0 && g.form.shape.is_none() && g.cavity.is_none()
}

/// The ground, sampled once per position.
struct Ground<'a> {
    land: &'a dyn Land,
    seen: FxHashMap<IVec2, GeoSample>,
}

impl Ground<'_> {
    fn at(&mut self, p: IVec2) -> GeoSample {
        if let Some(g) = self.seen.get(&p) {
            return *g;
        }
        let g = self.land.sample(p.x, p.y);
        self.seen.insert(p, g);
        g
    }
}

/// A rectangle of blocks, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub min: IVec2,
    pub max: IVec2,
}

impl Rect {
    fn new(min: IVec2, size: IVec2) -> Self {
        Rect { min, max: min + size - 1 }
    }

    /// Blocks between the two (negative where they overlap).
    fn gap(&self, other: &Rect) -> i32 {
        let dx = (other.min.x - self.max.x).max(self.min.x - other.max.x) - 1;
        let dz = (other.min.y - self.max.y).max(self.min.y - other.max.y) - 1;
        dx.max(dz)
    }

    fn distance(&self, p: Vec2) -> f32 {
        let lo = self.min.as_vec2() - 0.5;
        let hi = self.max.as_vec2() + 0.5;
        let d = (lo - p).max(p - hi).max(Vec2::ZERO);
        d.length()
    }

    fn grow(&self, by: i32) -> Rect {
        Rect { min: self.min - by, max: self.max + by }
    }

    fn spanning(a: IVec2, b: IVec2) -> Rect {
        Rect { min: a.min(b), max: a.max(b) }
    }
}

/// A lane: its centre line and half its width.
#[derive(Clone, Debug)]
pub struct Lane {
    pub points: Vec<Vec2>,
    pub half: f32,
    pub paved: bool,
    /// Whether it leads out of the settlement, a trail joining its end.
    pub main: bool,
}

impl Lane {
    fn distance(&self, p: Vec2) -> f32 {
        self.points.windows(2).map(|w| segment_distance(p, w[0], w[1])).fold(f32::MAX, f32::min).min(self.points.first().map_or(f32::MAX, |q| q.distance(p)))
    }

    fn rect_distance(&self, rect: &Rect) -> f32 {
        let mut best = f32::MAX;
        for w in self.points.windows(2) {
            let n = (w[0].distance(w[1]).ceil() as i32).max(1);
            for i in 0..=n {
                best = best.min(rect.distance(w[0].lerp(w[1], i as f32 / n as f32)));
            }
        }
        best
    }

    fn length(&self) -> f32 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// The point `s` blocks along, and the direction there.
    fn along(&self, s: f32) -> (Vec2, Vec2) {
        let mut left = s;
        for w in self.points.windows(2) {
            let l = w[0].distance(w[1]);
            if left <= l && l > 0.0 {
                return (w[0].lerp(w[1], left / l), (w[1] - w[0]) / l);
            }
            left -= l;
        }
        let n = self.points.len();
        let dir = if n >= 2 { (self.points[n - 1] - self.points[n - 2]).normalize_or_zero() } else { Vec2::X };
        (self.points[n - 1], dir)
    }
}

fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Where in its square of the grid a settlement stands: the best of a few likely spots,
/// each scored on how gentle and dry it is, with fresh water near, and a roll of luck.
fn site(ctx: &Ctx, cell: IVec2, rng: &mut Rng) -> Option<(IVec2, bool)> {
    let grid = &ctx.def.grid;
    let first = cell * grid.spacing + grid.margin;
    let size = (grid.spacing - 2 * grid.margin) / STEP + 1;
    let samples = ctx.land.grid(first, STEP, size);
    let at = |i: i32, j: i32| -> Option<&GeoSample> { (i >= 0 && j >= 0 && i < size && j < size).then(|| &samples[(j * size + i) as usize]) };
    let reach = SITE_RADIUS / STEP;
    let mut candidates: Vec<(f32, IVec2, bool)> = Vec::new();
    for j in (reach..size - reach).step_by(2) {
        for i in (reach..size - reach).step_by(2) {
            let centre = at(i, j)?;
            let mut heights = Vec::with_capacity(32);
            let mut bad = false;
            for dj in -reach..=reach {
                for di in -reach..=reach {
                    if di * di + dj * dj > reach * reach + 1 {
                        continue;
                    }
                    let Some(g) = at(i + di, j + dj) else { continue };
                    if !firm(g) {
                        bad = true;
                    }
                    heights.push(g.height);
                }
            }
            if bad || heights.is_empty() {
                continue;
            }
            let mean = heights.iter().sum::<i32>() as f32 / heights.len() as f32;
            let range = heights.iter().max().unwrap() - heights.iter().min().unwrap();
            let site = &ctx.def.site;
            if mean < (SEA_LEVEL + site.height.0) as f32 || mean > (SEA_LEVEL + site.height.1) as f32 || range > site.relief {
                continue;
            }
            let spread = (heights.iter().map(|&h| (h as f32 - mean).powi(2)).sum::<f32>() / heights.len() as f32).sqrt();
            // Fresh water a little way off: a river or lake within reach, not underfoot.
            let mut fresh = false;
            for dj in -reach * 3..=reach * 3 {
                for di in -reach * 3..=reach * 3 {
                    if let Some(g) = at(i + di, j + dj)
                        && (g.submerged() || g.river > 0.5)
                        && g.body != crate::Body::Sea
                    {
                        fresh = true;
                    }
                }
            }
            let luck = unit(hash3(ctx.seed ^ 0x517e, first.x + i * STEP, 0, first.y + j * STEP)) * 3.0;
            let score = -spread * 1.2 - range as f32 * 0.25 + if fresh { 3.0 } else { 0.0 } + luck - (centre.height - SEA_LEVEL) as f32 * 0.02;
            candidates.push((score, first + IVec2::new(i, j) * STEP, fresh));
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then((a.1.x, a.1.y).cmp(&(b.1.x, b.1.y))));
    let _ = rng;
    for &(_, at, fresh) in candidates.iter().take(6) {
        let biome = ctx.world.biome_at(at.x, at.y);
        if ctx.def.avoid.iter().any(|t| biome.has_tag(t)) {
            continue;
        }
        let site = ctx.world.site(at.x, at.y);
        let (cold, hot) = ctx.def.site.temperature;
        if !(cold..=hot).contains(&site.temperature) {
            continue;
        }
        // Not beside a sinkhole or a ravine, which would open under its buildings.
        if ctx.world.caves.opens_near((at.x, at.y), ctx.def.site.caves) {
            continue;
        }
        return Some((at, fresh));
    }
    None
}

/// The character of a settlement at `at`: wealth from fertile soil, fresh water and gentle
/// ground, age from luck alone, order following wealth a little, crowding from luck and
/// green from the plants around.
fn levers(ctx: &Ctx, at: IVec2, fresh: bool, rng: &mut Rng) -> Levers {
    let site = ctx.world.site(at.x, at.y);
    let flat = 1.0 - (site.steepness / 0.6).min(1.0);
    let wealth = (0.45 * rng.unit() + 0.25 * site.fertility + if fresh { 0.2 } else { 0.0 } + 0.1 * flat).clamp(0.0, 1.0);
    let age = (rng.unit() + rng.unit()) / 2.0;
    let order = (0.65 * rng.unit() + 0.35 * wealth).clamp(0.0, 1.0);
    let crowding = (0.75 * rng.unit() + 0.25 * wealth).clamp(0.0, 1.0);
    let green = (0.6 * rng.unit() + 0.4 * site.forest.max(site.vegetation * 0.6)).clamp(0.0, 1.0);
    let mut l = [0.0; 5];
    l[Lever::Wealth as usize] = wealth;
    l[Lever::Age as usize] = age;
    l[Lever::Order as usize] = order;
    l[Lever::Crowding as usize] = crowding;
    l[Lever::Green as usize] = green;
    Levers(l)
}

/// The materials a settlement keeps of its style: a few of each list, by weight.
struct Kit {
    wood: String,
    foundation: Palette,
    floor: Vec<(BlockId, f32)>,
    walls: Vec<(BlockId, f32)>,
    frame: Vec<(BlockId, f32)>,
    roof: Vec<(BlockId, f32)>,
    trims: Vec<(BlockId, f32)>,
    paving: Palette,
    hedge: Palette,
    weathered: Palette,
    crops: Vec<[BlockId; 4]>,
    roofs: Vec<(Roof, f32)>,
    window: Option<BlockId>,
}

fn kit(ctx: &Ctx, style: &StyleDef, levers: &Levers, rng: &mut Rng) -> Kit {
    let woods: Vec<(usize, f32)> = style.woods.iter().enumerate().map(|(i, (_, w))| (i, *w)).collect();
    let wood = style.woods[pick(rng, &woods).unwrap_or(0)].0.clone();
    let trim = if rng.chance(0.55) { wood.clone() } else { style.woods[pick(rng, &woods).unwrap_or(0)].0.clone() };
    let resolve = |list: &[(String, f32)]| -> Vec<(BlockId, f32)> {
        list.iter().filter_map(|(name, w)| ctx.resolved.get(&fill(name, &wood, &trim)).map(|id| (id, *w))).filter(|(_, w)| *w > 0.0).collect()
    };
    // Orderly places keep fewer materials, and so look of a piece.
    let keep = |rng: &mut Rng, all: Vec<(BlockId, f32)>| -> Vec<(BlockId, f32)> {
        let most = 1 + ((1.0 - levers.get(Lever::Order)) * 2.5 * rng.unit()).round() as usize;
        let mut left = all;
        let mut kept = Vec::new();
        while kept.len() < most && !left.is_empty() {
            let indexed: Vec<(usize, f32)> = left.iter().enumerate().map(|(i, (_, w))| (i, *w)).collect();
            let Some(i) = pick(rng, &indexed) else { break };
            kept.push(left.remove(i));
        }
        kept
    };
    let crops = style
        .crops
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .filter_map(|(crop, _)| {
            let stage = |s: &str| ctx.resolved.get(&format!("{crop}{s}"));
            Some([stage("_crop_0")?, stage("_crop_1")?, stage("_crop_2")?, stage("_crop")?])
        })
        .collect();
    Kit {
        foundation: Palette(resolve(&style.foundation.0)),
        floor: keep(rng, resolve(&style.floor.0)),
        walls: keep(rng, resolve(&style.walls.0)),
        frame: keep(rng, resolve(&style.frame.0)),
        roof: keep(rng, resolve(&style.roof.0)),
        trims: keep(rng, resolve(&style.trim.0)),
        paving: Palette(resolve(&style.paving.0)),
        hedge: Palette(resolve(&style.hedge.0)),
        weathered: Palette(resolve(&style.weathered.0)),
        crops,
        // The style's roofs, each weighed again by how the place leans.
        roofs: style
            .roofs
            .iter()
            .map(|&(shape, w)| (shape, w * ctx.def.roofs.iter().find(|r| r.shape == shape).map_or(1.0, |r| r.lean.weigh(r.weight, levers))))
            .collect(),
        window: style.window.as_deref().and_then(|w| ctx.resolved.get(w)),
        wood,
    }
}

impl Kit {
    /// One building's materials, from those kept.
    fn mats(&self, ctx: &Ctx, rng: &mut Rng) -> Mats {
        let one = |rng: &mut Rng, list: &[(BlockId, f32)], or: BlockId| pick(rng, list).unwrap_or(or);
        let fallback = self.foundation.0.first().map_or(ctx.fixed.dirt, |b| b.0);
        let wall = one(rng, &self.walls, fallback);
        let log = ctx.resolved.get(&format!("base:{}_log", self.wood)).unwrap_or(fallback);
        Mats {
            foundation: self.foundation.clone(),
            floor: one(rng, &self.floor, wall),
            wall,
            frame: one(rng, &self.frame, wall),
            roof: one(rng, &self.roof, wall),
            trim: one(rng, &self.trims, wall),
            window: self.window,
            weathered: self.weathered.clone(),
            hedge: self.hedge.clone(),
            log,
            crops: self.crops.clone(),
        }
    }
}

impl Lot {
    /// The blocks it takes: its own, and the two before its door, kept free to walk in.
    fn taken(&self) -> impl Iterator<Item = Rect> {
        let step = self.frame.facing.step();
        let approach = self.door.map(|(d, _)| Rect::spanning(d + step, d + step * 2));
        std::iter::once(self.rect).chain(approach)
    }
}

/// What planning a lot needs to know about it.
struct Lot {
    spec: Spec,
    rect: Rect,
    frame: Frame,
    floor: i32,
    mats: Mats,
    /// Where its door opens onto, and the lane point it leads to.
    door: Option<(IVec2, Vec2)>,
    /// Blocks of yard kept clear around it.
    yard: i32,
    /// Which of the `houses` it is.
    house: Option<usize>,
    /// Which of the `centrepieces` it is.
    centre: Option<usize>,
}

/// Plans the settlement of the grid square `cell`, if it has one.
pub fn plan(ctx: &Ctx, cell: IVec2) -> Option<Plan> {
    let def = ctx.def;
    let mut rng = Rng::new(ctx.seed, 0x5e771e, cell.x, cell.y);
    if !rng.chance(def.grid.chance) {
        return None;
    }
    let (centre, fresh) = site(ctx, cell, &mut rng)?;
    let levers = levers(ctx, centre, fresh, &mut rng);
    let biome = ctx.world.biome_at(centre.x, centre.y);
    // Styles weigh by how many of their tags the biome has, so where two fit (a cool
    // forest with conifers) either may be built; the catch-all style stands in temperate
    // and cool land, and wherever nothing else fits.
    let tagged = |tags: &[String]| tags.is_empty() || tags.iter().any(|t| biome.has_tag(t));
    let matches = |s: &StyleDef| s.biome.iter().filter(|t| biome.has_tag(t)).count() as f32;
    let fitted = def.styles.iter().any(|s| matches(s) > 0.0);
    let styles: Vec<(usize, f32)> = def
        .styles
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w = if !s.biome.is_empty() {
                matches(s)
            } else if !fitted || biome.has_tag("temperate") || biome.has_tag("cool") {
                1.0
            } else {
                0.0
            };
            (i, w * w)
        })
        .collect();
    let style_index = pick(&mut rng, &styles)?;
    let style = &def.styles[style_index];
    let sizes: Vec<(usize, f32)> = def.sizes.iter().enumerate().map(|(i, s)| (i, s.lean.weigh(s.weight, &levers))).collect();
    let size_index = pick(&mut rng, &sizes)?;
    let size = &def.sizes[size_index];
    let layouts: Vec<(usize, f32)> = def.layouts.iter().enumerate().filter(|(_, l)| fresh || !l.water).map(|(i, l)| (i, l.lean.weigh(l.weight, &levers))).collect();
    let layout_index = pick(&mut rng, &layouts)?;
    let shape = def.layouts[layout_index].shape;
    let kit = kit(ctx, style, &levers, &mut rng);
    let names = def.names.iter().find(|n| tagged(&n.biome)).unwrap_or(&def.names[0]);
    let name = {
        let first = &names.first[rng.below(names.first.len() as u32) as usize];
        let small = size_index == 0 && !names.small.is_empty() && rng.chance(names.small_chance);
        let name = if small {
            let word = &names.small[rng.below(names.small.len() as u32) as usize];
            (format!("{} {}", first.0, word.0), format!("{}・{}", first.1, word.1))
        } else {
            // Never "Brookbrook".
            let mut last = &names.last[rng.below(names.last.len() as u32) as usize];
            for _ in 0..4 {
                let (a, b) = (first.0.to_lowercase(), last.0.to_lowercase());
                if !a.contains(&b) && !b.contains(&a) {
                    break;
                }
                last = &names.last[rng.below(names.last.len() as u32) as usize];
            }
            (format!("{}{}", first.0, last.0), format!("{}{}", first.1, last.1))
        };
        // Now and then a word before it, leaning on the levers ("Old" where it is old).
        let prefixes: Vec<(usize, f32)> = names.prefixes.iter().enumerate().map(|(i, p)| (i, p.3.weigh(p.2, &levers))).collect();
        match (rng.chance(names.prefix_chance), pick(&mut rng, &prefixes)) {
            (true, Some(i)) => {
                let p = &names.prefixes[i];
                (format!("{} {}", p.0, name.0), format!("{}・{}", p.1, name.1))
            }
            _ => name,
        }
    };

    let mut ground = Ground { land: ctx.land, seen: FxHashMap::default() };
    let square_height = ground.at(centre).height;
    let order = levers.get(Lever::Order);
    let crowding = levers.get(Lever::Crowding);
    let wealth = levers.get(Lever::Wealth);
    let village = size.always.contains(&Special::Inn);
    let mut square = match shape {
        Shape::Scatter => 2,
        Shape::Green => between(&mut rng, size.square) * 3 / 2 + 2,
        _ => between(&mut rng, size.square),
    };
    // The one great thing it is known by, if it has one: on the square, which grows to
    // hold it and the well beside it, or out at the end of a lane.
    let centres: Vec<(usize, f32)> = def.centrepieces.iter().enumerate().map(|(i, c)| (i, c.lean.weigh(c.weight, &levers))).collect();
    let centrepiece = if rng.chance(size.centrepiece) { pick(&mut rng, &centres) } else { None };
    let centre_spec = centrepiece.map(|i| centre_spec(def, i, &kit, &levers, &mut rng, style));
    let on_square = centrepiece.is_some_and(|i| def.centrepieces[i].spot == Spot::Square);
    if on_square && let Some(spec) = &centre_spec {
        square = square.max(spec.w.max(spec.d) / 2 + 5).min(16);
    }
    let bounds = Rect { min: cell * def.grid.spacing + def.grid.margin / 2, max: (cell + 1) * def.grid.spacing - def.grid.margin / 2 - 1 };

    // Lanes.
    let mut lanes: Vec<Lane> = Vec::new();
    let lane_count = between(&mut rng, size.lanes);
    let length = || -> f32 { 0.0 };
    let _ = length;
    let main_half = if village { 1.4 } else { 0.9 };
    let side_half = 0.8;
    let paved = village && wealth > 0.55;
    let wiggle = 0.06 + 0.32 * (1.0 - order);
    let start_angle = rng.unit() * std::f32::consts::TAU;
    let lane_length = |rng: &mut Rng| between(rng, size.lane_length) as f32;
    let walk_from = |ground: &mut Ground, lanes: &mut Vec<Lane>, rng: &mut Rng, from: Vec2, angle: f32, length: f32, half: f32, main: bool| {
        let points = walk(ground, lanes, rng, from, angle, length, wiggle, &bounds);
        if points.len() >= 3 {
            lanes.push(Lane { points, half, paved: paved && main, main });
        }
    };
    let ring = square as f32 + 0.5;
    let from_square = |angle: f32| centre.as_vec2() + Vec2::from_angle(angle) * ring;
    // A straight lane, where the ground between its ends is firm.
    let straight = |ground: &mut Ground, from: Vec2, to: Vec2, half: f32| -> Option<Lane> {
        let n = (from.distance(to) / 2.0).ceil().max(1.0) as i32;
        (0..=n).all(|i| firm(&ground.at(from.lerp(to, i as f32 / n as f32).round().as_ivec2()))).then(|| Lane { points: vec![from, to], half, paved: false, main: false })
    };
    // Side lanes off the lanes there are, `count` of them, on the side `away` points to if
    // any.
    let branch = |ground: &mut Ground, lanes: &mut Vec<Lane>, rng: &mut Rng, count: i32, away: Option<Vec2>| {
        let street: Vec<Lane> = lanes.clone();
        for k in 0..count {
            let Some(lane) = street.get((k as usize) % street.len().max(1)) else { break };
            let s = lane.length() * (0.35 + 0.4 * rng.unit());
            let (p, dir) = lane.along(s);
            let mut side = if k % 2 == 0 { 1.0 } else { -1.0 };
            if let Some(away) = away
                && Vec2::new(-dir.y, dir.x).dot(away) * side < 0.0
            {
                side = -side;
            }
            let angle = dir.to_angle() + side * std::f32::consts::FRAC_PI_2 + (rng.unit() - 0.5) * 0.4 * (1.0 - order);
            let l = lane_length(rng) * 0.5;
            walk_from(ground, lanes, rng, p, angle, l, side_half, false);
        }
    };
    match shape {
        Shape::Spokes | Shape::Green | Shape::Scatter => {
            for k in 0..lane_count {
                let spread = if shape == Shape::Scatter { 1.0 } else { 0.35 * (1.0 - order) };
                let angle = start_angle + k as f32 * std::f32::consts::TAU / lane_count as f32 + (rng.unit() - 0.5) * spread;
                let l = lane_length(&mut rng);
                walk_from(&mut ground, &mut lanes, &mut rng, from_square(angle), angle, l, main_half, true);
            }
            if shape == Shape::Scatter {
                // Paths branch off the paths.
                let branches = lanes.len();
                for k in 0..branches {
                    if rng.chance(0.6) {
                        let lane = lanes[k].clone();
                        let s = lane.length() * (0.3 + 0.5 * rng.unit());
                        let (p, dir) = lane.along(s);
                        let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
                        let angle = dir.to_angle() + side * (0.9 + 0.6 * rng.unit());
                        let l = lane_length(&mut rng) * 0.6;
                        walk_from(&mut ground, &mut lanes, &mut rng, p, angle, l, side_half, false);
                    }
                }
            }
        }
        Shape::Line | Shape::Fork => {
            let l = lane_length(&mut rng) * 1.2;
            walk_from(&mut ground, &mut lanes, &mut rng, from_square(start_angle), start_angle, l, main_half, true);
            let back = start_angle + std::f32::consts::PI + (rng.unit() - 0.5) * 0.3;
            if shape == Shape::Fork {
                // The street runs out one way, then splits in two.
                let l2 = lane_length(&mut rng) * 0.5;
                let before = lanes.len();
                walk_from(&mut ground, &mut lanes, &mut rng, from_square(back), back, l2, main_half, true);
                if lanes.len() > before {
                    let lane = lanes.last().unwrap().clone();
                    let (end, dir) = lane.along(lane.length());
                    for side in [-1.0f32, 1.0] {
                        let angle = dir.to_angle() + side * (0.5 + 0.4 * rng.unit());
                        let l = lane_length(&mut rng) * 0.8;
                        walk_from(&mut ground, &mut lanes, &mut rng, end, angle, l, main_half, true);
                    }
                    // The stem no longer leads out.
                    lanes[before].main = false;
                }
            } else {
                let l2 = lane_length(&mut rng) * 1.2;
                walk_from(&mut ground, &mut lanes, &mut rng, from_square(back), back, l2, main_half, true);
                // Side lanes off the street.
                branch(&mut ground, &mut lanes, &mut rng, lane_count.saturating_sub(2), None);
            }
        }
        Shape::Crossroads => {
            // Four ways meeting square on, side lanes off them.
            for k in 0..4 {
                let angle = start_angle + k as f32 * std::f32::consts::FRAC_PI_2 + (rng.unit() - 0.5) * 0.2 * (1.0 - order);
                let l = lane_length(&mut rng);
                walk_from(&mut ground, &mut lanes, &mut rng, from_square(angle), angle, l, main_half, true);
            }
            branch(&mut ground, &mut lanes, &mut rng, lane_count.saturating_sub(3), None);
        }
        Shape::Ring => {
            // A lane round the square at a house's depth or two, houses both sides of it,
            // two paths in from the square and lanes leading out.
            let radius = square as f32 + 10.0 + 4.0 * (1.0 - crowding);
            let n = (radius * std::f32::consts::TAU / 3.0).ceil() as usize;
            let wobble = 1.2 * (1.0 - order);
            let mut points = Vec::with_capacity(n + 1);
            let mut whole = true;
            for k in 0..=n {
                let a = start_angle + k as f32 * std::f32::consts::TAU / n as f32;
                let r = radius + (unit(hash3(ctx.seed ^ 0x5149, cell.x, (k % n) as i32, cell.y)) - 0.5) * 2.0 * wobble;
                let p = centre.as_vec2() + Vec2::from_angle(a) * r;
                let g = ground.at(p.round().as_ivec2());
                if !firm(&g) || (g.height - square_height).abs() > 6 {
                    whole = false;
                    break;
                }
                points.push(p);
            }
            let reach = if whole {
                lanes.push(Lane { points, half: side_half + 0.2, paved: false, main: false });
                for k in 0..2 {
                    let a = start_angle + 0.4 + k as f32 * std::f32::consts::PI;
                    if let Some(lane) = straight(&mut ground, from_square(a), centre.as_vec2() + Vec2::from_angle(a) * radius, side_half) {
                        lanes.push(lane);
                    }
                }
                radius
            } else {
                ring
            };
            let out = lane_count.max(2);
            for k in 0..out {
                let angle = start_angle + 1.2 + k as f32 * std::f32::consts::TAU / out as f32 + (rng.unit() - 0.5) * 0.3;
                let l = lane_length(&mut rng) * 0.7;
                walk_from(&mut ground, &mut lanes, &mut rng, centre.as_vec2() + Vec2::from_angle(angle) * reach, angle, l, main_half, true);
            }
        }
        Shape::Riverside => {
            // A street along the bank of the water near, lanes off it away from the water.
            let toward = water_near(&mut ground, centre, 48);
            let along = toward.map_or(start_angle, |t| t.to_angle() + std::f32::consts::FRAC_PI_2);
            let l = lane_length(&mut rng) * 1.2;
            walk_from(&mut ground, &mut lanes, &mut rng, from_square(along), along, l, main_half, true);
            let back = along + std::f32::consts::PI;
            let l2 = lane_length(&mut rng) * 1.2;
            walk_from(&mut ground, &mut lanes, &mut rng, from_square(back), back, l2, main_half, true);
            branch(&mut ground, &mut lanes, &mut rng, lane_count.saturating_sub(2), toward.map(|t| -t));
        }
        Shape::Terraces => {
            // Streets along the slope, one above another, joined by paths up from the
            // square.
            let uphill = slope(&mut ground, centre).unwrap_or_else(|| Vec2::from_angle(start_angle));
            let along = uphill.to_angle() + std::f32::consts::FRAC_PI_2;
            for angle in [along, along + std::f32::consts::PI] {
                let l = lane_length(&mut rng);
                walk_from(&mut ground, &mut lanes, &mut rng, from_square(angle), angle, l, main_half, true);
            }
            let step = 11.0 + 4.0 * (1.0 - crowding);
            for side in [-1.0f32, 1.0] {
                let up = uphill * side;
                let to = centre.as_vec2() + up * (ring + step);
                let Some(path) = straight(&mut ground, centre.as_vec2() + up * ring, to, side_half) else { continue };
                lanes.push(path);
                for angle in [along, along + std::f32::consts::PI] {
                    let l = lane_length(&mut rng) * 0.7;
                    walk_from(&mut ground, &mut lanes, &mut rng, to, angle, l, side_half, false);
                }
            }
        }
    }
    if lanes.is_empty() {
        return None;
    }

    // What to build: the size's own places, then houses.
    let mut wanted: Vec<Special> = size.always.clone();
    for &(special, chance) in &size.sometimes {
        if rng.chance(chance) {
            wanted.push(special);
        }
    }
    let houses = between(&mut rng, size.houses);
    let mut lots: Vec<Lot> = Vec::new();
    let mut occupied: Vec<Rect> = Vec::new();
    let square_rect = Rect { min: centre - square, max: centre + square };
    let gap = 1 + ((1.0 - crowding) * 3.0).round() as i32;

    // A centrepiece on the square stands at its heart, the well beside it.
    let mut well_at = centre;
    if on_square
        && let Some(spec) = centre_spec.clone()
    {
        let extent = IVec2::new(spec.w, spec.d);
        let rect = Rect::new(centre - extent / 2, extent);
        let mats = kit.mats(ctx, &mut rng);
        let frame = Frame::fit(IVec2::ZERO, extent - 1, Facing::North, rect.min, square_height);
        occupied.push(rect);
        lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0, house: None, centre: centrepiece });
        let side = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y][rng.below(4) as usize];
        well_at = centre + side * (extent.max_element() / 2 + 3);
    }
    // The well at the heart.
    if wanted.contains(&Special::Well) {
        let size = if village && rng.chance(0.5) { 4 } else { 3 };
        let spec = special_spec(Kind::Well, size, size, &kit, &levers, &mut rng, style);
        let rect = Rect::new(well_at - size / 2, IVec2::splat(size));
        let mats = kit.mats(ctx, &mut rng);
        let frame = Frame::fit(IVec2::ZERO, IVec2::new(size - 1, size - 1), Facing::North, rect.min, square_height);
        occupied.push(rect);
        lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0, house: None, centre: None });
    }
    // Stalls round the square, facing in.
    let stalls = wanted.iter().filter(|s| **s == Special::Stall).count();
    for k in 0..stalls {
        let angle = start_angle + 0.5 + k as f32 * std::f32::consts::TAU / stalls.max(1) as f32 + rng.unit() * 0.4;
        let p = centre.as_vec2() + Vec2::from_angle(angle) * (square as f32 - 2.0);
        let facing = Facing::toward(centre.as_vec2() - p);
        let spec = special_spec(Kind::Stall, 3, 2 + rng.below(2), &kit, &levers, &mut rng, style);
        let (low, high) = spec.envelope();
        let extent = Frame::extent(low, high, facing);
        let rect = Rect::new(p.round().as_ivec2() - extent / 2, extent);
        if occupied.iter().any(|o| o.gap(&rect) < 1) || lanes.iter().any(|l| l.rect_distance(&rect) < l.half + 0.5) {
            continue;
        }
        let mats = kit.mats(ctx, &mut rng);
        let frame = Frame::fit(low, high, facing, rect.min, square_height);
        occupied.push(rect);
        lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0, house: None, centre: None });
    }

    // The rest go along the lanes: grand places nearest the square, houses after, and
    // fields, pens, orchards and woodyards out at the ends.
    let mut queue: Vec<Kind> = Vec::new();
    for s in &wanted {
        match s {
            Special::Inn => queue.push(Kind::Inn),
            Special::Smithy => queue.push(Kind::Smithy),
            Special::Trader => queue.push(Kind::Trader),
            _ => {}
        }
    }
    for _ in 0..houses {
        queue.push(Kind::House);
    }
    let mut outskirts: Vec<Kind> = Vec::new();
    for s in &wanted {
        match s {
            Special::Field => outskirts.push(Kind::Field),
            Special::Pen => outskirts.push(Kind::Pen),
            Special::Orchard => outskirts.push(Kind::Orchard),
            Special::Woodyard => outskirts.push(Kind::Woodyard),
            Special::Shrine => outskirts.push(Kind::Shrine),
            _ => {}
        }
    }

    let mut town = Town { ctx, kit: &kit, levers: &levers, style, size: &size.name, built: FxHashMap::default() };
    // On a green, houses first take the rim, facing in.
    if shape == Shape::Green {
        let rim = (square as f32 + 4.0 + gap as f32).max(8.0);
        let mut angle = start_angle;
        let mut tries = 0;
        while tries < 24 && !queue.is_empty() && queue.len() > houses as usize / 2 {
            tries += 1;
            angle += 0.45 + rng.unit() * 0.5;
            let p = centre.as_vec2() + Vec2::from_angle(angle) * rim;
            let kind = queue[0];
            if let Some(lot) = place(&mut town, &mut ground, &mut rng, kind, None, p, Facing::toward(centre.as_vec2() - p), &occupied, &lanes, &square_rect, gap, None) {
                occupied.extend(lot.taken());
                town.count(&lot);
                lots.push(lot);
                queue.remove(0);
            }
        }
    }
    let mut spacing = 7.0 + 6.0 * (1.0 - crowding);
    for pass in 0..3 {
        if queue.is_empty() {
            break;
        }
        for li in 0..lanes.len() {
            let lane = lanes[li].clone();
            let total = lane.length();
            let mut s = 3.0 + rng.unit() * 3.0 + pass as f32 * 2.0;
            let mut side = if rng.chance(0.5) { 1.0f32 } else { -1.0 };
            while s < total && !queue.is_empty() {
                let (p, dir) = lane.along(s);
                let normal = Vec2::new(-dir.y, dir.x) * side;
                let kind = queue[0];
                let setback = lane.half + 1.5 + ((1.0 - crowding) * 3.0 * rng.unit()).round();
                let front = p + normal * setback;
                if let Some(lot) = place(&mut town, &mut ground, &mut rng, kind, None, front, Facing::toward(-normal), &occupied, &lanes, &square_rect, gap, Some(p)) {
                    occupied.extend(lot.taken());
                    town.count(&lot);
                    lots.push(lot);
                    queue.remove(0);
                    s += spacing * (0.6 + 0.8 * rng.unit());
                } else {
                    s += 2.0;
                }
                // Orderly places alternate sides; others wander.
                if order > 0.5 || rng.chance(0.5) {
                    side = -side;
                }
            }
        }
        spacing *= 0.7;
    }
    // Out at the ends of the lanes, and a centrepiece that stands there.
    let edge = centre_spec.filter(|_| !on_square);
    let outskirts: Vec<(Kind, Option<Spec>)> = edge.into_iter().map(|s| (Kind::Centrepiece, Some(s))).chain(outskirts.into_iter().map(|k| (k, None))).collect();
    for (kind, given) in outskirts {
        let mut placed = false;
        for _ in 0..10 {
            let lane = &lanes[rng.below(lanes.len() as u32) as usize];
            let total = lane.length();
            let s = total * (0.55 + 0.45 * rng.unit());
            let (p, dir) = lane.along(s);
            let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
            let normal = Vec2::new(-dir.y, dir.x) * side;
            let front = p + normal * (lane.half + 2.0 + rng.unit() * 4.0);
            if let Some(mut lot) = place(&mut town, &mut ground, &mut rng, kind, given.clone(), front, Facing::toward(-normal), &occupied, &lanes, &square_rect, gap, None) {
                if kind == Kind::Centrepiece {
                    lot.centre = centrepiece;
                }
                occupied.extend(lot.taken());
                lots.push(lot);
                placed = true;
                break;
            }
        }
        let _ = placed;
    }
    if lots.iter().filter(|l| l.spec.kind.indoors()).count() < 2 {
        return None;
    }

    // Props by the lanes and in the yards, and by the water.
    let mut canvas = Canvas::default();
    let mut props: Vec<(Rect, usize, Facing)> = Vec::new();
    let prop_weights: Vec<(usize, f32)> = def.props.iter().enumerate().filter(|(_, p)| !p.water).map(|(i, p)| (i, p.lean.weigh(p.weight, &levers))).collect();
    let water_weights: Vec<(usize, f32)> = def.props.iter().enumerate().filter(|(_, p)| p.water).map(|(i, p)| (i, p.lean.weigh(p.weight, &levers))).collect();
    let lamp = def.props.iter().position(|p| p.kind == PropKind::Drawn(Prop::Lamp));
    // Where a prop would stand with its first block at `at`, if the ground there is free.
    let fit = |ground: &mut Ground, occupied: &[Rect], prop: usize, at: IVec2, facing: Facing, clearance: f32| -> Option<Rect> {
        let (low, high) = prop_box(&def.props[prop].kind);
        let rect = match &def.props[prop].kind {
            PropKind::Drawn(_) => Rect::new(at, Frame::extent(low, high, facing)),
            PropKind::Blocks(..) => Rect { min: at + low, max: at + high },
        };
        let free = occupied.iter().all(|o| o.gap(&rect) >= 0)
            && lanes.iter().all(|l| l.rect_distance(&rect) > l.half + clearance)
            && [rect.min, rect.max, IVec2::new(rect.min.x, rect.max.y), IVec2::new(rect.max.x, rect.min.y)].iter().all(|&c| firm(&ground.at(c)));
        free.then_some(rect)
    };
    let mut lamps_every = 9.0 + 14.0 * (1.0 - wealth);
    if !village {
        lamps_every *= 1.5;
    }
    for lane in &lanes {
        let total = lane.length();
        let mut s = 4.0 + rng.unit() * lamps_every;
        while s < total {
            let (p, dir) = lane.along(s);
            let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
            let at = (p + Vec2::new(-dir.y, dir.x) * side * (lane.half + 1.0)).round().as_ivec2();
            let facing = Facing::toward(p - at.as_vec2());
            let prop = if rng.chance(0.5 + 0.4 * wealth) && lamp.is_some() { lamp } else { pick(&mut rng, &prop_weights) };
            if let Some(prop) = prop
                && let Some(rect) = fit(&mut ground, &occupied, prop, at, facing, 0.4)
            {
                props.push((rect, prop, facing));
                occupied.push(rect);
            }
            s += lamps_every * (0.7 + 0.6 * rng.unit());
        }
    }
    // A few more in yards.
    let yard_props = (lots.len() as f32 * (0.5 + 1.0 * levers.get(Lever::Green)) * rng.unit()).round() as usize + 2;
    for _ in 0..yard_props * 3 {
        if props.len() > yard_props + lanes.len() * 6 {
            break;
        }
        let lot = &lots[rng.below(lots.len() as u32) as usize];
        if !lot.spec.kind.indoors() {
            continue;
        }
        let r = lot.rect.grow(1 + rng.below(2));
        let at = match rng.below(4) {
            0 => IVec2::new(rng.range(r.min.x, r.max.x + 1), r.min.y),
            1 => IVec2::new(rng.range(r.min.x, r.max.x + 1), r.max.y),
            2 => IVec2::new(r.min.x, rng.range(r.min.y, r.max.y + 1)),
            _ => IVec2::new(r.max.x, rng.range(r.min.y, r.max.y + 1)),
        };
        let facing = [Facing::North, Facing::East, Facing::South, Facing::West][rng.below(4) as usize];
        if let Some(prop) = pick(&mut rng, &prop_weights)
            && let Some(rect) = fit(&mut ground, &occupied, prop, at, facing, 0.6)
        {
            props.push((rect, prop, facing));
            occupied.push(rect);
        }
    }
    // Boats and nets on the bank of the water near.
    if fresh && !water_weights.is_empty() {
        let reach = lots.iter().map(|l| l.rect.distance(centre.as_vec2()) as i32).max().unwrap_or(0) + 8;
        let mut banks = 1 + rng.below(3);
        for spot in banks_near(&mut ground, centre, reach) {
            if banks == 0 {
                break;
            }
            let Some(prop) = pick(&mut rng, &water_weights) else { break };
            let facing = Facing::toward(spot.1);
            if let Some(rect) = fit(&mut ground, &occupied, prop, spot.0, facing, 0.6) {
                props.push((rect, prop, facing));
                occupied.push(rect);
                banks -= 1;
            }
        }
    }

    // Draw everything.
    for lot in &lots {
        match (lot.spec.kind, lot.centre) {
            (Kind::Centrepiece, Some(i)) => {
                let c = &def.centrepieces[i];
                let named = |part: &str| c.parts.iter().find(|(p, _)| p == part).and_then(|(_, name)| ctx.resolved.get(&fill(name, &kit.wood, &kit.wood)));
                match &c.kind {
                    CentreKind::Drawn(drawn) => {
                        let parts = centre::Parts::new(&named, &lot.mats, ctx.fixed, lot.spec.seed);
                        centre::draw(*drawn, &lot.spec, &lot.frame, &lot.mats, ctx.fixed, &parts, &mut canvas);
                    }
                    CentreKind::Blocks(_, blocks) => {
                        let half = (lot.spec.w / 2, lot.spec.d / 2);
                        for (offset, name) in blocks {
                            if let Some(block) = ctx.resolved.get(name) {
                                canvas.set(lot.frame.at(half.0 + offset.x, offset.y, half.1 + offset.z), block);
                            }
                        }
                    }
                }
            }
            _ => house::draw(&lot.spec, &lot.frame, &lot.mats, ctx.fixed, &mut canvas),
        }
    }
    let prop_mats = kit.mats(ctx, &mut rng);
    for &(rect, prop, facing) in &props {
        match &def.props[prop].kind {
            PropKind::Drawn(drawn) => {
                let (low, high) = prop_box(&def.props[prop].kind);
                let frame = Frame::fit(low, high, facing, rect.min, 0);
                draw_prop(ctx.fixed, &prop_mats, &mut canvas, &frame, *drawn, hash3(ctx.seed ^ 0x9409, rect.min.x, 0, rect.min.y));
            }
            PropKind::Blocks(_, blocks) => {
                let (low, _) = prop_box(&def.props[prop].kind);
                let at = rect.min - low;
                for (offset, name) in blocks {
                    if let Some(block) = ctx.resolved.get(name) {
                        canvas.on_ground(at.x + offset.x, at.y + offset.z, offset.y, Put::Set(block));
                    }
                }
            }
        }
    }
    // Paving: the square, and the main lanes of rich villages.
    let pave_square = shape != Shape::Green && (village || wealth > 0.45) && !kit.paving.is_empty();
    let seed = hash3(ctx.seed, cell.x, 7, cell.y);
    if pave_square {
        let r = square as f32 + 0.5;
        for z in -square - 1..=square + 1 {
            for x in -square - 1..=square + 1 {
                let p = centre + IVec2::new(x, z);
                let d = p.as_vec2().distance(centre.as_vec2());
                let ragged = unit(hash3(seed ^ 0x5a, p.x, 0, p.y)) * 1.6 * (1.0 - order + 0.3);
                // Not under a centrepiece, a pond or a fountain least of all.
                let under = lots.iter().any(|l| l.spec.kind == Kind::Centrepiece && l.rect.distance(p.as_vec2()) < 0.5);
                if d <= r - ragged
                    && !under
                    && let Some(block) = kit.paving.at(seed, IVec3::new(p.x, 0, p.y))
                {
                    canvas.on_ground(p.x, p.y, 0, Put::Set(block));
                }
            }
        }
    }

    // The ground: pads graded flat under buildings and the square, lanes trodden.
    let mut buildings = Vec::new();
    let mut min = centre - square - 2;
    let mut max = centre + square + 2;
    for lane in &lanes {
        for p in &lane.points {
            min = min.min(p.floor().as_ivec2() - 4);
            max = max.max(p.ceil().as_ivec2() + 4);
        }
    }
    for lot in &lots {
        min = min.min(lot.rect.min - (FALLOFF + 4).max(def.site.glade.1));
        max = max.max(lot.rect.max + (FALLOFF + 4).max(def.site.glade.1));
    }
    for (rect, ..) in &props {
        min = min.min(rect.min - 2);
        max = max.max(rect.max + 2);
    }
    let mut raster = Raster::new(min, max);
    // The square: flat in the middle, easing out.
    raster.pad(&Rect { min: centre - square, max: centre + square }, square_height, true, FALLOFF + 1, 0, |p| p.as_vec2().distance(centre.as_vec2()) <= square as f32 + 0.5);
    for lot in &lots {
        let grade = !matches!(lot.spec.kind, Kind::Stall | Kind::Shrine | Kind::Pen | Kind::Orchard | Kind::Woodyard);
        let core = lot.rect.grow(if grade { 1 } else { 0 });
        if grade {
            raster.pad(&core, lot.floor, false, FALLOFF, lot.yard, |_| true);
        } else {
            raster.clear_rect(&lot.rect.grow(lot.yard));
        }
        buildings.push(Building {
            kind: lot.spec.kind,
            min: lot.rect.min,
            max: lot.rect.max,
            floor: lot.floor + lot.spec.stilts,
            // Two blocks out from the door, on the step below the top one when it stands
            // on stilts.
            door: lot.door.map(|(d, _)| IVec3::new(d.x, lot.floor + (lot.spec.stilts - 1).max(0) + 1, d.y)),
            facing: lot.frame.facing,
            what: match (lot.house, lot.centre) {
                (Some(i), _) => def.houses[i].name.clone(),
                (_, Some(i)) => match &def.centrepieces[i].kind {
                    CentreKind::Drawn(c) => format!("{c:?}"),
                    CentreKind::Blocks(name, _) => name.clone(),
                },
                _ => lot.spec.kind.name().to_string(),
            },
            work: lot.house.and_then(|i| def.houses[i].work.clone()),
        });
    }
    raster.finish();
    for lane in &lanes {
        raster.lane(lane);
    }
    // A path from each door to its lane.
    for lot in &lots {
        if let Some((door, to)) = lot.door {
            let from = door.as_vec2();
            let path = Lane { points: vec![from, to], half: 0.5, paved: false, main: false };
            raster.lane(&path);
        }
    }
    for (rect, ..) in &props {
        raster.clear_rect(rect);
    }
    // A clearing in the woods, leafier where the place is green.
    let rects: Vec<Rect> = lots.iter().map(|l| l.rect).collect();
    let (near, far) = def.site.glade;
    let (trees, greener) = def.site.glade_trees;
    raster.glade(&rects, near, far, trees + greener * levers.get(Lever::Green), seed);
    // Paved lanes.
    for lane in lanes.iter().filter(|l| l.paved) {
        for z in raster.min.y..=raster.max.y {
            for x in raster.min.x..=raster.max.x {
                let p = IVec2::new(x, z);
                if lane.distance(p.as_vec2()) <= lane.half
                    && let Some(block) = kit.paving.at(seed, IVec3::new(x, 0, z))
                {
                    canvas.grounded.entry(IVec3::new(x, 0, z)).or_insert(Put::Set(block));
                }
            }
        }
    }

    let gates: Vec<IVec2> = lanes.iter().filter(|l| l.main).map(|l| l.points.last().unwrap().round().as_ivec2()).collect();
    let size_name = &def.sizes[size_index].name;
    let people = super::people::residents(&def.people, &def.houses, size_name, &levers, &buildings, &gates, hash3(ctx.seed, cell.x, 7, cell.y));
    Some(Plan {
        id: hash3(ctx.seed, cell.x, 0, cell.y),
        size: def.sizes[size_index].name.clone(),
        layout: def.layouts[layout_index].name.clone(),
        style: style.name.clone(),
        name,
        centre,
        height: square_height,
        levers,
        buildings,
        gates,
        lanes: lanes.iter().map(|l| l.points.clone()).collect(),
        people,
        min: raster.min,
        max: raster.max,
        raster,
        voxels: canvas.into_columns(),
    })
}

/// How far graded ground eases back into the land around, in blocks.
pub const FALLOFF: i32 = 5;


/// Walks a lane from `from`, heading `angle`, for `length` blocks: it bends a little at
/// random, more in disorderly places, and steers round water, steep ground and other
/// lanes, ending early where it cannot go on.
#[allow(clippy::too_many_arguments)]
fn walk(ground: &mut Ground, lanes: &[Lane], rng: &mut Rng, from: Vec2, angle: f32, length: f32, wiggle: f32, bounds: &Rect) -> Vec<Vec2> {
    const STEP: f32 = 3.0;
    let mut points = vec![from];
    let mut heading = angle;
    let mut bend = 0.0f32;
    let mut here = from;
    let mut height = ground.at(from.round().as_ivec2()).height;
    let mut walked = 0.0;
    while walked < length {
        bend = (bend + (rng.unit() - 0.5) * wiggle * 0.5).clamp(-wiggle, wiggle);
        heading += bend;
        let mut best: Option<(f32, f32, Vec2, i32)> = None;
        for turn in [-0.45f32, -0.2, 0.0, 0.2, 0.45] {
            let a = heading + turn;
            let next = here + Vec2::from_angle(a) * STEP;
            let at = next.round().as_ivec2();
            if at.x < bounds.min.x || at.y < bounds.min.y || at.x > bounds.max.x || at.y > bounds.max.y {
                continue;
            }
            let g = ground.at(at);
            if !firm(&g) {
                continue;
            }
            let rise = (g.height - height).abs();
            if rise > 2 {
                continue;
            }
            // Not into another lane, past its first few blocks.
            if walked > 6.0 && lanes.iter().any(|l| l.distance(next) < l.half + 3.5) {
                continue;
            }
            let cost = rise as f32 * 1.5 + turn.abs() * 1.2;
            if best.is_none_or(|b| cost < b.0) {
                best = Some((cost, a, next, g.height));
            }
        }
        let Some((_, a, next, h)) = best else { break };
        heading = a;
        here = next;
        height = h;
        points.push(here);
        walked += STEP;
    }
    points
}

/// Rolls a special building's spec.
fn special_spec(kind: Kind, w: i32, d: i32, kit: &Kit, levers: &Levers, rng: &mut Rng, style: &StyleDef) -> Spec {
    let roof = match pick(rng, &kit.roofs).unwrap_or(Roof::Gable) {
        Roof::HalfHip | Roof::Saltbox | Roof::Gambrel => Roof::Gable,
        Roof::Conical => Roof::Hip,
        other => other,
    };
    Spec {
        kind,
        w,
        d,
        wall: 3,
        storeys: 1,
        roof,
        pitch: 1,
        ridge_x: w > d,
        extras: Vec::new(),
        door: w / 2,
        shed: 0,
        wing: None,
        stilts: 0,
        regular: levers.get(Lever::Order) > 0.5,
        window_chance: style.window_chance,
        form: Form::Box,
        inside: Vec::new(),
        centre: None,
        seed: rng.next_u64(),
    }
}

/// A centrepiece's spec: its size, and what the code draws.
fn centre_spec(def: &SettlementsDef, index: usize, kit: &Kit, levers: &Levers, rng: &mut Rng, style: &StyleDef) -> Spec {
    let (w, d, drawn) = match &def.centrepieces[index].kind {
        CentreKind::Drawn(c) => {
            let (w, d) = c.extent();
            (w, d, Some(*c))
        }
        CentreKind::Blocks(_, blocks) => {
            let half = blocks.iter().map(|(at, _)| at.x.abs().max(at.z.abs())).max().unwrap_or(0);
            (half * 2 + 1, half * 2 + 1, None)
        }
    };
    let mut spec = special_spec(Kind::Centrepiece, w, d, kit, levers, rng, style);
    spec.centre = drawn;
    spec
}

/// The direction of the nearest fresh water within `reach` blocks, if any.
fn water_near(ground: &mut Ground, centre: IVec2, reach: i32) -> Option<Vec2> {
    for r in (8..=reach).step_by(8) {
        let mut sum = Vec2::ZERO;
        for k in 0..24 {
            let dir = Vec2::from_angle(k as f32 * std::f32::consts::TAU / 24.0);
            let g = ground.at(centre + (dir * r as f32).round().as_ivec2());
            if (g.submerged() || g.river > 0.5) && g.body != crate::Body::Sea {
                sum += dir;
            }
        }
        if sum != Vec2::ZERO {
            return Some(sum.normalize_or(Vec2::X));
        }
    }
    None
}

/// Uphill at `centre`, where the ground slopes at all.
fn slope(ground: &mut Ground, centre: IVec2) -> Option<Vec2> {
    let h = |ground: &mut Ground, dx: i32, dz: i32| ground.at(centre + IVec2::new(dx, dz)).height as f32;
    let grad = Vec2::new(h(ground, 16, 0) - h(ground, -16, 0), h(ground, 0, 16) - h(ground, 0, -16));
    (grad.length() >= 2.0).then(|| grad.normalize())
}

/// Firm spots on the bank of fresh water within `reach` of `centre`, nearest first, each
/// with the way to the water.
fn banks_near(ground: &mut Ground, centre: IVec2, reach: i32) -> Vec<(IVec2, Vec2)> {
    let mut found = Vec::new();
    for r in (10..=reach.max(12)).step_by(3) {
        let n = (r as f32 * std::f32::consts::TAU / 4.0) as i32;
        for k in 0..n {
            let at = centre + (Vec2::from_angle(k as f32 * std::f32::consts::TAU / n as f32) * r as f32).round().as_ivec2();
            if !firm(&ground.at(at)) {
                continue;
            }
            for (dx, dz) in [(2, 0), (-2, 0), (0, 2), (0, -2)] {
                let g = ground.at(at + IVec2::new(dx, dz));
                if g.submerged() && g.body != crate::Body::Sea {
                    found.push((at, Vec2::new(dx as f32, dz as f32)));
                    break;
                }
            }
        }
        if found.len() >= 8 {
            break;
        }
    }
    found
}

/// A prop's own box, from its first block: (0, 0) to its extent less one for one the code
/// draws, the bounds of its blocks for one of a package's own.
fn prop_box(kind: &PropKind) -> (IVec2, IVec2) {
    match kind {
        PropKind::Drawn(p) => {
            let (w, d) = p.extent();
            (IVec2::ZERO, IVec2::new(w - 1, d - 1))
        }
        PropKind::Blocks(_, blocks) => {
            let mut low = IVec2::ZERO;
            let mut high = IVec2::ZERO;
            for (at, _) in blocks {
                low = low.min(IVec2::new(at.x, at.z));
                high = high.max(IVec2::new(at.x, at.z));
            }
            (low, high)
        }
    }
}

/// What placing buildings needs to know of the settlement: its kit, levers and style, its
/// size by name, and how many of each house it has built.
struct Town<'a> {
    ctx: &'a Ctx<'a>,
    kit: &'a Kit,
    levers: &'a Levers,
    style: &'a StyleDef,
    size: &'a str,
    built: FxHashMap<usize, u32>,
}

impl Town<'_> {
    fn count(&mut self, lot: &Lot) {
        if let Some(i) = lot.house {
            *self.built.entry(i).or_default() += 1;
        }
    }
}

/// Rolls a house of the settlement's kinds, with its extras: one the place's size builds,
/// of which it has not built its most, beside water if it must be. Inns are the grandest
/// homes of four walls; traders keep a modest home or a shop.
fn house_spec(town: &Town, kind: Kind, rng: &mut Rng, near_water: bool) -> (Spec, usize) {
    let (def, kit, levers, style) = (town.ctx.def, town.kit, town.levers, town.style);
    let fits = |i: usize, h: &super::def::HouseDef| {
        (h.sizes.is_empty() || h.sizes.iter().any(|s| s == town.size))
            && h.most.is_none_or(|most| town.built.get(&i).copied().unwrap_or(0) < most)
            && (!h.water || near_water)
            && match kind {
                Kind::Inn => h.work.is_none() && h.form == Form::Box,
                Kind::Trader => h.work.is_none() && matches!(h.form, Form::Box | Form::ShopFront),
                _ => true,
            }
    };
    let houses: Vec<(usize, f32)> = def.houses.iter().enumerate().filter(|(i, h)| fits(*i, h)).map(|(i, h)| (i, h.lean.weigh(h.weight, levers))).collect();
    let mut index = pick(rng, &houses).unwrap_or(0);
    // The inn is the grandest house in the place, the trader's a modest one.
    if kind == Kind::Inn {
        index = def.houses.iter().enumerate().filter(|(i, h)| fits(*i, h) && h.width.1 >= 8).max_by_key(|(_, h)| h.width.1).map_or(index, |(i, _)| i);
    }
    let h = &def.houses[index];
    let mut w = between(rng, h.width);
    let mut d = between(rng, h.depth);
    let mut storeys = between(rng, h.storeys);
    let mut wall = between(rng, h.walls);
    if kind == Kind::Inn {
        storeys = storeys.max(2);
        w = w.max(8);
        d = d.max(9);
    }
    if kind == Kind::Trader {
        w = w.clamp(5, 7);
        d = d.clamp(5, 8);
    }
    let mut extras: Vec<Extra> = Vec::new();
    for e in &def.extras {
        if rng.chance(e.lean.weigh(e.chance, levers)) {
            extras.push(e.name);
        }
    }
    for &e in &h.always {
        if !extras.contains(&e) {
            extras.push(e);
        }
    }
    if rng.chance(h.porch) {
        extras.push(Extra::Porch);
    }
    // What each form has no room for.
    let unfit: &[Extra] = match h.form {
        Form::Box => &[],
        Form::Round => &[Extra::Porch, Extra::Shed, Extra::Wing, Extra::Balcony, Extra::OutsideStair, Extra::Dormer, Extra::Awning],
        Form::Courtyard => &[Extra::Porch, Extra::Shed, Extra::Wing, Extra::Balcony, Extra::OutsideStair, Extra::Dormer],
        Form::AFrame => &[Extra::Porch, Extra::Shed, Extra::Wing, Extra::Balcony, Extra::OutsideStair, Extra::Dormer, Extra::Awning, Extra::Shutters, Extra::HangingHerbs, Extra::SignBoard],
        Form::ShopFront => &[Extra::Porch, Extra::Awning],
    };
    extras.retain(|e| !unfit.contains(e));
    match h.form {
        Form::Round => {
            w -= 1 - w % 2;
            d = w;
        }
        Form::Courtyard => {
            w = w.max(11);
            d = d.max(11);
        }
        Form::AFrame => {
            storeys = 1;
            wall = 0;
        }
        Form::ShopFront => storeys = storeys.max(2),
        Form::Box => {}
    }
    if storeys < 2 {
        extras.retain(|e| !matches!(e, Extra::Balcony | Extra::OutsideStair));
    }
    if kind == Kind::Inn {
        extras.retain(|e| *e != Extra::Porch);
        extras.push(Extra::Chimney);
    }
    if kind == Kind::Trader {
        extras.retain(|e| !matches!(e, Extra::Porch | Extra::Awning));
    }
    let roofs = if h.roofs.is_empty() { &kit.roofs } else { &h.roofs };
    let roof = match (h.form, pick(rng, roofs).unwrap_or(Roof::Gable)) {
        (Form::Round | Form::Courtyard, Roof::Flat) => Roof::Flat,
        (Form::Round, _) => Roof::Conical,
        (Form::Courtyard, _) => Roof::Hip,
        (Form::AFrame, _) => Roof::Gable,
        (_, roof) => roof,
    };
    // Low roofs now and then on big houses; cones, barns and A-frames are steep.
    let steep = matches!(roof, Roof::Flat | Roof::Conical | Roof::Gambrel) || h.form == Form::AFrame;
    let pitch = if !steep && w.max(d) >= 8 && rng.chance(0.45) { 2 } else { 1 };
    let boxy = matches!(h.form, Form::Box | Form::ShopFront);
    let wing = (boxy && (rng.chance(h.wing) || extras.contains(&Extra::Wing))).then(|| (3 + rng.below(2), 3 + rng.below(3))).filter(|(ww, _)| *ww < w - 1);
    let shed = if boxy && extras.contains(&Extra::Shed) && d >= 5 { 3 + rng.below((d - 4).min(3) as u32) } else { 0 };
    let door = if levers.get(Lever::Order) > 0.6 || !boxy { w / 2 } else { rng.range(1, w - 1) };
    let ridge_x = match (h.form, roof) {
        (Form::AFrame, _) => false,
        (_, Roof::Saltbox) => true,
        _ => {
            if rng.chance(0.85) {
                w > d
            } else {
                w <= d
            }
        }
    };
    let spec = Spec {
        kind,
        w,
        d,
        wall,
        storeys,
        roof,
        pitch,
        ridge_x,
        extras,
        door,
        shed,
        wing,
        stilts: h.stilts.unwrap_or(style.stilts) as i32,
        regular: levers.get(Lever::Order) > 0.45,
        window_chance: (style.window_chance * (0.6 + 0.8 * levers.get(Lever::Wealth))).min(0.95),
        form: h.form,
        inside: h.inside.clone(),
        centre: None,
        seed: rng.next_u64(),
    };
    (spec, index)
}

/// Whether fresh water lies within a few blocks of `at`.
fn wet(ground: &mut Ground, at: Vec2) -> bool {
    let at = at.round().as_ivec2();
    (0..8).any(|k| {
        let g = ground.at(at + (Vec2::from_angle(k as f32 * std::f32::consts::FRAC_PI_4) * 7.0).round().as_ivec2());
        (g.submerged() || g.river > 0.5) && g.body != crate::Body::Sea
    })
}

/// Tries to place a building of `kind` (or `given`) with its front at `front`, facing
/// `facing`; `lane` is where its path joins the lane.
#[allow(clippy::too_many_arguments)]
fn place(
    town: &mut Town,
    ground: &mut Ground,
    rng: &mut Rng,
    kind: Kind,
    given: Option<Spec>,
    front: Vec2,
    facing: Facing,
    occupied: &[Rect],
    lanes: &[Lane],
    square: &Rect,
    gap: i32,
    lane: Option<Vec2>,
) -> Option<Lot> {
    let (ctx, kit, levers, style) = (town.ctx, town.kit, town.levers, town.style);
    let mut house = None;
    let spec = match (given, kind) {
        (Some(spec), _) => spec,
        (None, Kind::House | Kind::Trader | Kind::Inn) => {
            let near_water = ctx.def.houses.iter().any(|h| h.water) && wet(ground, front);
            let (spec, index) = house_spec(town, kind, rng, near_water);
            house = Some(index);
            spec
        }
        (None, Kind::Smithy) => special_spec(Kind::Smithy, 6 + rng.below(3), 5 + rng.below(3), kit, levers, rng, style),
        (None, Kind::Field) => special_spec(Kind::Field, 7 + rng.below(3) * 2, 7 + rng.below(7), kit, levers, rng, style),
        (None, Kind::Pen) => special_spec(Kind::Pen, 6 + rng.below(5), 6 + rng.below(4), kit, levers, rng, style),
        (None, Kind::Orchard) => special_spec(Kind::Orchard, 9 + rng.below(5), 9 + rng.below(5), kit, levers, rng, style),
        (None, Kind::Woodyard) => special_spec(Kind::Woodyard, 4 + rng.below(3), 3 + rng.below(3), kit, levers, rng, style),
        (None, Kind::Shrine) => special_spec(Kind::Shrine, 3, 3, kit, levers, rng, style),
        (None, Kind::Well | Kind::Stall | Kind::Centrepiece) => special_spec(kind, 3, 3, kit, levers, rng, style),
    };
    let (low, high) = spec.envelope();
    let extent = Frame::extent(low, high, facing);
    // The envelope's front edge on `front`, centred across.
    let f = front.round().as_ivec2();
    let min = match facing {
        Facing::North => IVec2::new(f.x - extent.x / 2, f.y),
        Facing::South => IVec2::new(f.x - extent.x / 2, f.y - extent.y + 1),
        Facing::West => IVec2::new(f.x, f.y - extent.y / 2),
        Facing::East => IVec2::new(f.x - extent.x + 1, f.y - extent.y / 2),
    };
    let rect = Rect::new(min, extent);
    if occupied.iter().any(|o| o.gap(&rect) < gap) || square.gap(&rect) < 1 {
        return None;
    }
    if lanes.iter().any(|l| l.rect_distance(&rect) < l.half + 0.6) {
        return None;
    }
    // Gentle, firm ground under it all.
    let mut heights = Vec::with_capacity(16);
    let step = IVec2::new((extent.x - 1).max(1) / 3, (extent.y - 1).max(1) / 3).max(IVec2::ONE);
    let mut z = rect.min.y;
    while z <= rect.max.y {
        let mut x = rect.min.x;
        while x <= rect.max.x {
            let g = ground.at(IVec2::new(x, z));
            if !firm(&g) {
                return None;
            }
            heights.push(g.height);
            x += step.x;
        }
        z += step.y;
    }
    heights.sort_unstable();
    let range = heights[heights.len() - 1] - heights[0];
    let limit = match kind {
        Kind::Field | Kind::Orchard | Kind::Centrepiece => 3,
        Kind::Pen | Kind::Woodyard | Kind::Shrine => 5,
        _ => 4 + spec.stilts,
    };
    if range > limit {
        return None;
    }
    let floor = heights[heights.len() / 2];
    if let Some(at) = lane {
        let road = ground.at(at.round().as_ivec2()).height;
        if (road - floor).abs() > 3 {
            return None;
        }
    }
    let frame = Frame::fit(low, high, facing, rect.min, floor);
    let door = kind.indoors().then(|| {
        let outside = frame.at(spec.door, 0, -2);
        (IVec2::new(outside.x, outside.z), lane.unwrap_or(front - facing.step().as_vec2() * 2.0))
    });
    let door = door.or_else(|| {
        (kind == Kind::Smithy).then(|| {
            let outside = frame.at(spec.w / 2, 0, -1);
            (IVec2::new(outside.x, outside.z), lane.unwrap_or(front))
        })
    });
    let mats = kit.mats(ctx, rng);
    let yard = (1 + ((1.0 - levers.get(Lever::Crowding)) * 2.0) as i32).min(gap);
    Some(Lot { spec, rect, frame, floor, mats, door, yard, house, centre: None })
}

/// Draws a prop in its frame, standing on the ground.
pub(super) fn draw_prop(fixed: &Fixed, mats: &Mats, canvas: &mut Canvas, frame: &Frame, prop: Prop, seed: u64) {
    let origin = frame.at(0, 0, 0);
    let u = |salt: u64| unit(hash3(seed ^ salt, origin.x, 0, origin.z));
    let mut put = |x: i32, up: i32, z: i32, put: Put| {
        let p = frame.at(x, 0, z);
        canvas.on_ground(p.x, p.z, up, put);
    };
    let cloth = |salt: u64| fixed.cloth.pick(u(salt)).or(fixed.canopies.pick(u(salt))).unwrap_or(mats.trim);
    let barrel = fixed.barrel.unwrap_or(mats.log);
    let hay = fixed.hay.pick(u(0x4a)).unwrap_or(mats.trim);
    match prop {
        Prop::Lamp => {
            put(0, 1, 0, Put::Set(mats.frame));
            put(0, 2, 0, Put::Set(mats.frame));
            if u(1) < 0.4 {
                put(0, 3, 0, Put::Set(mats.frame));
                put(0, 4, 0, Put::Set(fixed.torch));
            } else {
                put(0, 3, 0, Put::Set(fixed.torch));
            }
        }
        Prop::Woodpile => {
            put(0, 1, 0, Put::Soft(mats.log));
            if u(2) < 0.6 {
                put(0, 2, 0, Put::Soft(mats.log));
            }
        }
        Prop::Crates => {
            put(0, 1, 0, Put::Soft(mats.trim));
            if u(3) < 0.4 {
                put(0, 2, 0, Put::Soft(mats.trim));
            }
        }
        Prop::StonePile => {
            if let Some(b) = fixed.boulder.pick(u(4)) {
                put(0, 1, 0, Put::Soft(b));
            }
        }
        Prop::FlowerBed => {
            if let Some(f) = fixed.flowers.pick(u(5)) {
                put(0, 1, 0, Put::Soft(f));
            }
        }
        Prop::HerbPlanter => {
            if let Some(p) = fixed.planter {
                put(0, 1, 0, Put::Soft(p));
            }
        }
        Prop::Campfire => put(0, 1, 0, Put::Soft(fixed.campfire)),
        Prop::Bush => {
            if let Some(b) = fixed.bushes.pick(u(6)).or(mats.hedge.pick(u(6))) {
                put(0, 1, 0, Put::Soft(b));
            }
        }
        Prop::Compost => {
            if let Some(b) = fixed.compost.pick(u(7)) {
                put(0, 1, 0, Put::Soft(b));
            }
        }
        Prop::Boulder => {
            if let Some(b) = fixed.boulder.pick(u(8)) {
                put(0, 1, 0, Put::Soft(b));
                if u(9) < 0.3 {
                    put(0, 2, 0, Put::Soft(b));
                }
            }
        }
        Prop::Cart => {
            // Handles to the ground in front, wheels under the middle, a load on top.
            put(0, 1, 0, Put::Set(mats.trim));
            put(1, 1, 0, Put::Set(mats.trim));
            for z in 1..=2 {
                for x in 0..=1 {
                    put(x, 2, z, Put::Set(mats.trim));
                    put(x, 1, z, if z == 1 { Put::Set(mats.log) } else { Put::Clear });
                }
            }
            let load = match (u(0xca) * 3.0) as i32 {
                0 => Some(hay),
                1 => fixed.sacks.pick(u(0xcb)),
                _ => None,
            };
            if let Some(load) = load {
                put(0, 3, 2, Put::Soft(load));
                if u(0xcc) < 0.5 {
                    put(1, 3, 1, Put::Soft(load));
                }
            }
        }
        Prop::Barrel => {
            put(0, 1, 0, Put::Soft(barrel));
            if u(0xba) < 0.3 {
                put(0, 2, 0, Put::Soft(barrel));
            }
        }
        Prop::HayBale => {
            put(0, 1, 0, Put::Soft(hay));
            if u(0x4b) < 0.4 {
                put(0, 2, 0, Put::Soft(hay));
            }
        }
        Prop::LaundryLine => {
            for x in [0, 4] {
                for up in 1..=3 {
                    put(x, up, 0, Put::Set(mats.frame));
                }
            }
            for x in 1..=3 {
                if u(0x1a + x as u64) < 0.8 {
                    put(x, 3, 0, Put::Set(cloth(0x1b + x as u64)));
                    if u(0x1c + x as u64) < 0.35 {
                        put(x, 2, 0, Put::Set(cloth(0x1b + x as u64)));
                    }
                }
            }
        }
        Prop::FenceRun => {
            for x in 0..4 {
                let post = x == 0 || x == 3;
                put(x, 1, 0, if post { Put::Set(mats.frame) } else { Put::Clear });
                put(x, 2, 0, Put::Set(if post { mats.frame } else { mats.trim }));
            }
        }
        Prop::Scarecrow => {
            for up in 1..=3 {
                put(1, up, 0, Put::Set(mats.frame));
            }
            put(0, 3, 0, Put::Set(mats.trim));
            put(2, 3, 0, Put::Set(mats.trim));
            put(1, 4, 0, Put::Set(hay));
        }
        Prop::Skep => {
            put(0, 1, 0, Put::Set(mats.log));
            put(0, 2, 0, Put::Set(hay));
        }
        Prop::Anvil => {
            put(0, 1, 0, Put::Set(mats.log));
            put(0, 2, 0, Put::Set(fixed.anvil));
        }
        Prop::Grindstone => {
            put(0, 1, 0, Put::Set(mats.frame));
            put(1, 1, 0, Put::Set(mats.stone()));
            put(0, 2, 0, Put::Set(mats.trim));
        }
        Prop::Washtub => {
            put(0, 0, 0, Put::Set(fixed.water));
            put(1, 1, 0, Put::Soft(mats.trim));
        }
        Prop::Sacks => {
            for x in 0..=1 {
                if let Some(sack) = fixed.sacks.pick(u(0x5a + x as u64)) {
                    put(x, 1, 0, Put::Soft(sack));
                    if x == 0 && u(0x5c) < 0.4 {
                        put(x, 2, 0, Put::Soft(sack));
                    }
                }
            }
        }
        Prop::ChickenCoop => {
            for (x, z) in [(0, 0), (0, 1), (1, 1)] {
                put(x, 1, z, Put::Set(mats.trim));
            }
            put(1, 1, 0, Put::Clear);
            for z in 0..=1 {
                for x in 0..=1 {
                    put(x, 2, z, Put::Set(mats.roof));
                }
            }
        }
        Prop::Signpost => {
            put(0, 1, 0, Put::Set(mats.frame));
            put(0, 2, 0, Put::Set(mats.frame));
            put(0, 3, 0, Put::Set(mats.trim));
        }
        Prop::Boat => {
            // Drawn up on the bank, its bow to the water.
            for z in 0..4 {
                for x in 0..=1 {
                    put(x, 1, z, Put::Set(mats.trim));
                }
            }
            put(0, 2, 3, Put::Set(mats.trim));
            put(1, 2, 3, Put::Set(mats.trim));
        }
        Prop::Nets => {
            for x in [0, 2] {
                put(x, 1, 0, Put::Set(mats.frame));
                put(x, 2, 0, Put::Set(mats.frame));
            }
            for x in 0..=2 {
                put(x, 3, 0, Put::Set(mats.trim));
            }
            if let Some(net) = fixed.hanging.pick(u(0x4e)) {
                put(1, 2, 0, Put::Set(net));
            }
        }
    }
}
