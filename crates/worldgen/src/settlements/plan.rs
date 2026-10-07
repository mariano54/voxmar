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
use super::def::{Extra, Lean, Lever, Levers, Prop, PropKind, Roof, SettlementsDef, Shape, Special, StyleDef, fill};
use super::house::{self, Fixed, Kind, Mats, Spec};
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
        roofs: style.roofs.clone(),
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
    let layouts: Vec<(usize, f32)> = def.layouts.iter().enumerate().map(|(i, l)| (i, l.lean.weigh(l.weight, &levers))).collect();
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
    let square = match shape {
        Shape::Scatter => 2,
        Shape::Green => between(&mut rng, size.square) * 3 / 2 + 2,
        _ => between(&mut rng, size.square),
    };
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
                let street: Vec<Lane> = lanes.clone();
                for k in 0..lane_count.saturating_sub(2) {
                    let lane = &street[(k as usize) % street.len().max(1)];
                    let s = lane.length() * (0.35 + 0.4 * rng.unit());
                    let (p, dir) = lane.along(s);
                    let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                    let angle = dir.to_angle() + side * std::f32::consts::FRAC_PI_2 + (rng.unit() - 0.5) * 0.4 * (1.0 - order);
                    let l = lane_length(&mut rng) * 0.5;
                    walk_from(&mut ground, &mut lanes, &mut rng, p, angle, l, side_half, false);
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

    // The well at the heart.
    if wanted.contains(&Special::Well) {
        let size = if village && rng.chance(0.5) { 4 } else { 3 };
        let spec = special_spec(Kind::Well, size, size, &kit, &levers, &mut rng, style);
        let rect = Rect::new(centre - size / 2, IVec2::splat(size));
        let mats = kit.mats(ctx, &mut rng);
        let frame = Frame::fit(IVec2::ZERO, IVec2::new(size - 1, size - 1), Facing::North, rect.min, square_height);
        occupied.push(rect);
        lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0 });
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
        lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0 });
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
            if let Some(lot) = place(ctx, &kit, &levers, style, &mut ground, &mut rng, kind, p, Facing::toward(centre.as_vec2() - p), &occupied, &lanes, &square_rect, gap, None) {
                occupied.extend(lot.taken());
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
                if let Some(lot) = place(ctx, &kit, &levers, style, &mut ground, &mut rng, kind, front, Facing::toward(-normal), &occupied, &lanes, &square_rect, gap, Some(p)) {
                    occupied.extend(lot.taken());
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
    // Out at the ends of the lanes.
    for kind in outskirts {
        let mut placed = false;
        for _ in 0..10 {
            let lane = &lanes[rng.below(lanes.len() as u32) as usize];
            let total = lane.length();
            let s = total * (0.55 + 0.45 * rng.unit());
            let (p, dir) = lane.along(s);
            let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
            let normal = Vec2::new(-dir.y, dir.x) * side;
            let front = p + normal * (lane.half + 2.0 + rng.unit() * 4.0);
            if let Some(lot) = place(ctx, &kit, &levers, style, &mut ground, &mut rng, kind, front, Facing::toward(-normal), &occupied, &lanes, &square_rect, gap, None) {
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

    // Props by the lanes and in the yards.
    let mut canvas = Canvas::default();
    let mut props: Vec<(IVec2, usize)> = Vec::new();
    let prop_weights: Vec<(usize, f32)> = def.props.iter().enumerate().map(|(i, p)| (i, p.lean.weigh(p.weight, &levers))).collect();
    let lamp = def.props.iter().position(|p| p.kind == PropKind::Drawn(Prop::Lamp));
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
            let spot = Rect::new(at, IVec2::ONE);
            let free = occupied.iter().all(|o| o.gap(&spot) >= 0) && lanes.iter().all(|l| l.distance(at.as_vec2()) > l.half + 0.4) && firm(&ground.at(at));
            if free {
                let prop = if rng.chance(0.5 + 0.4 * wealth) && lamp.is_some() { lamp } else { pick(&mut rng, &prop_weights) };
                if let Some(prop) = prop {
                    props.push((at, prop));
                    occupied.push(spot);
                }
            }
            s += lamps_every * (0.7 + 0.6 * rng.unit());
        }
    }
    // A few more in yards.
    let yard_props = (lots.len() as f32 * (0.4 + 0.8 * levers.get(Lever::Green)) * rng.unit()).round() as usize + 1;
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
        let spot = Rect::new(at, IVec2::ONE);
        if occupied.iter().all(|o| o.gap(&spot) >= 0) && lanes.iter().all(|l| l.distance(at.as_vec2()) > l.half + 0.6) && firm(&ground.at(at))
            && let Some(prop) = pick(&mut rng, &prop_weights)
        {
            props.push((at, prop));
            occupied.push(spot);
        }
    }

    // Draw everything.
    for lot in &lots {
        house::draw(&lot.spec, &lot.frame, &lot.mats, ctx.fixed, &mut canvas);
    }
    let prop_mats = kit.mats(ctx, &mut rng);
    for &(at, prop) in &props {
        match &def.props[prop].kind {
            PropKind::Drawn(drawn) => draw_prop(ctx, &prop_mats, &mut canvas, at, *drawn, hash3(ctx.seed ^ 0x9409, at.x, 0, at.y)),
            PropKind::Blocks(_, blocks) => {
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
                if d <= r - ragged
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
    for &(at, _) in &props {
        min = min.min(at - 2);
        max = max.max(at + 2);
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
            door: lot.door.map(|(d, _)| IVec3::new(d.x, lot.floor + lot.spec.stilts + 1, d.y)),
            facing: lot.frame.facing,
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
    for &(at, _) in &props {
        raster.clear_rect(&Rect::new(at, IVec2::ONE));
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
    let roofs: Vec<(Roof, f32)> = kit.roofs.clone();
    Spec {
        kind,
        w,
        d,
        wall: 3,
        storeys: 1,
        roof: pick(rng, &roofs).unwrap_or(Roof::Gable),
        pitch: 1,
        ridge_x: w > d,
        extras: Vec::new(),
        door: w / 2,
        shed: 0,
        wing: None,
        stilts: 0,
        regular: levers.get(Lever::Order) > 0.5,
        window_chance: style.window_chance,
        seed: rng.next_u64(),
    }
}

/// Rolls a house of the settlement's kinds, with its extras.
fn house_spec(ctx: &Ctx, kind: Kind, kit: &Kit, levers: &Levers, rng: &mut Rng, style: &StyleDef) -> Spec {
    let def = ctx.def;
    let houses: Vec<(usize, f32)> = def.houses.iter().enumerate().map(|(i, h)| (i, h.lean.weigh(h.weight, levers))).collect();
    let mut h = &def.houses[pick(rng, &houses).unwrap_or(0)];
    // The inn is the grandest house in the place, the trader's a modest one.
    if kind == Kind::Inn {
        h = def.houses.iter().filter(|h| h.width.1 >= 8).max_by_key(|h| h.width.1).unwrap_or(h);
    }
    let mut w = between(rng, h.width);
    let mut d = between(rng, h.depth);
    let mut storeys = between(rng, h.storeys);
    let wall = between(rng, h.walls);
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
    if rng.chance(h.porch) {
        extras.push(Extra::Porch);
    }
    if kind == Kind::Inn {
        extras.retain(|e| *e != Extra::Porch);
        extras.push(Extra::Chimney);
    }
    if kind == Kind::Trader {
        extras.retain(|e| *e != Extra::Porch);
    }
    let roof = pick(rng, &kit.roofs).unwrap_or(Roof::Gable);
    // Low roofs where it is warm and the style is open; steep ones elsewhere.
    let pitch = if roof != Roof::Flat && w.max(d) >= 8 && rng.chance(0.45) { 2 } else { 1 };
    let wing = (rng.chance(h.wing) || extras.contains(&Extra::Wing)).then(|| (3 + rng.below(2), 3 + rng.below(3))).filter(|(ww, _)| *ww < w - 1);
    let shed = if extras.contains(&Extra::Shed) && d >= 5 { 3 + rng.below((d - 4).min(3) as u32) } else { 0 };
    let door = if levers.get(Lever::Order) > 0.6 { w / 2 } else { rng.range(1, w - 1) };
    Spec {
        kind,
        w,
        d,
        wall,
        storeys,
        roof,
        pitch,
        ridge_x: if rng.chance(0.85) { w > d } else { w <= d },
        extras,
        door,
        shed,
        wing,
        stilts: style.stilts as i32,
        regular: levers.get(Lever::Order) > 0.45,
        window_chance: (style.window_chance * (0.6 + 0.8 * levers.get(Lever::Wealth))).min(0.95),
        seed: rng.next_u64(),
    }
}

/// Tries to place a building of `kind` with its front at `front`, facing `facing`;
/// `lane` is where its path joins the lane.
#[allow(clippy::too_many_arguments)]
fn place(
    ctx: &Ctx,
    kit: &Kit,
    levers: &Levers,
    style: &StyleDef,
    ground: &mut Ground,
    rng: &mut Rng,
    kind: Kind,
    front: Vec2,
    facing: Facing,
    occupied: &[Rect],
    lanes: &[Lane],
    square: &Rect,
    gap: i32,
    lane: Option<Vec2>,
) -> Option<Lot> {
    let spec = match kind {
        Kind::House | Kind::Trader | Kind::Inn => house_spec(ctx, kind, kit, levers, rng, style),
        Kind::Smithy => special_spec(Kind::Smithy, 6 + rng.below(3), 5 + rng.below(3), kit, levers, rng, style),
        Kind::Field => special_spec(Kind::Field, 7 + rng.below(3) * 2, 7 + rng.below(7), kit, levers, rng, style),
        Kind::Pen => special_spec(Kind::Pen, 6 + rng.below(5), 6 + rng.below(4), kit, levers, rng, style),
        Kind::Orchard => special_spec(Kind::Orchard, 9 + rng.below(5), 9 + rng.below(5), kit, levers, rng, style),
        Kind::Woodyard => special_spec(Kind::Woodyard, 4 + rng.below(3), 3 + rng.below(3), kit, levers, rng, style),
        Kind::Shrine => special_spec(Kind::Shrine, 3, 3, kit, levers, rng, style),
        Kind::Well | Kind::Stall => special_spec(kind, 3, 3, kit, levers, rng, style),
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
        Kind::Field | Kind::Orchard => 3,
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
    let door = door.or_else(|| (kind == Kind::Smithy).then(|| {
        let outside = frame.at(spec.w / 2, 0, -1);
        (IVec2::new(outside.x, outside.z), lane.unwrap_or(front))
    }));
    let mats = kit.mats(ctx, rng);
    let yard = (1 + ((1.0 - levers.get(Lever::Crowding)) * 2.0) as i32).min(gap);
    Some(Lot { spec, rect, frame, floor, mats, door, yard })
}

/// Draws a prop at `at`, standing on the ground.
fn draw_prop(ctx: &Ctx, mats: &Mats, canvas: &mut Canvas, at: IVec2, prop: Prop, seed: u64) {
    let fixed = ctx.fixed;
    let u = |salt: u64| unit(hash3(seed ^ salt, at.x, 0, at.y));
    let mut put = |up: i32, put: Put| canvas.on_ground(at.x, at.y, up, put);
    match prop {
        Prop::Lamp => {
            put(1, Put::Set(mats.frame));
            put(2, Put::Set(mats.frame));
            if u(1) < 0.4 {
                put(3, Put::Set(mats.frame));
                put(4, Put::Set(fixed.torch));
            } else {
                put(3, Put::Set(fixed.torch));
            }
        }
        Prop::Woodpile => {
            put(1, Put::Soft(mats.log));
            if u(2) < 0.6 {
                put(2, Put::Soft(mats.log));
            }
        }
        Prop::Crates => {
            put(1, Put::Soft(mats.trim));
            if u(3) < 0.4 {
                put(2, Put::Soft(mats.trim));
            }
        }
        Prop::StonePile => {
            if let Some(b) = fixed.boulder.pick(u(4)) {
                put(1, Put::Soft(b));
            }
        }
        Prop::FlowerBed => {
            if let Some(f) = fixed.flowers.pick(u(5)) {
                put(1, Put::Soft(f));
            }
        }
        Prop::HerbPlanter => {
            if let Some(p) = fixed.planter {
                put(1, Put::Soft(p));
            }
        }
        Prop::Campfire => {
            put(1, Put::Soft(fixed.campfire));
        }
        Prop::Bush => {
            if let Some(b) = fixed.bushes.pick(u(6)).or(mats.hedge.pick(u(6))) {
                put(1, Put::Soft(b));
            }
        }
        Prop::Compost => {
            if let Some(b) = fixed.compost.pick(u(7)) {
                put(1, Put::Soft(b));
            }
        }
        Prop::Boulder => {
            if let Some(b) = fixed.boulder.pick(u(8)) {
                put(1, Put::Soft(b));
                if u(9) < 0.3 {
                    put(2, Put::Soft(b));
                }
            }
        }
    }
}

// So `Lean` is used by name in docs.
#[allow(dead_code)]
fn _lean(_: &Lean) {}
