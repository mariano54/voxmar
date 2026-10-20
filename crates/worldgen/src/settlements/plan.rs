//! Planning one settlement: where in its square it stands, its character (levers), what
//! it is built of, its lanes, what stands along them, and the ground it grades.
//!
//! Everything here reads only the land with its rivers and lakes (no trails, which are
//! laid after settlements) and the climate, so a plan is a pure function of the seed and
//! its square.

use rustc_hash::FxHashMap;
use std::sync::Arc;
use voxmar_engine::block::BlockId;
use voxmar_engine::glam::{IVec2, IVec3, Vec2};

use super::canvas::{Canvas, Facing, Frame, Palette, Put, unit};
use super::culture::{Culture, Listing, Script, Site};
use super::def::{BYRE_KEEPS, CentreKind, Extra, Form, KanaJoin, LanesDef, Lever, Levers, Prop, PropKind, Roof, RulesDef, SettlementsDef, Shape, Special, Spot, StyleDef, fill};
use super::house::{self, Fixed, Kind, Mats, Spec};
use super::centre;
use super::{Building, Plan, Raster};
use crate::geology::GeoSample;
use crate::rng::{Rng, hash3, mix};
use crate::trails::Land;
use crate::{Overworld, SEA_LEVEL};

/// Why plans were given up, counted for the census in tests.
#[cfg(test)]
pub(super) static GIVEN_UP: [std::sync::atomic::AtomicU32; 12] = [const { std::sync::atomic::AtomicU32::new(0) }; 12];

#[cfg(test)]
pub(super) static PLACE_FAILS: [std::sync::atomic::AtomicU32; 8] = [const { std::sync::atomic::AtomicU32::new(0) }; 8];

/// Roof pieces laid in shingles because a flame reached them, and flames put out.
#[cfg(test)]
pub(super) static ROOFS_RELAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(test)]
pub(super) static FLAMES_PUT_OUT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Wall blocks left out where the ring runs against a lot, in every plan made (none should be).
#[cfg(test)]
pub(super) static WALL_ON_LOTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(test)]
thread_local! {
    static PROFILE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LANE_COUNTS: std::cell::Cell<[usize; 4]> = const { std::cell::Cell::new([0; 4]) };
    static ORIGINAL_ROWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Compare complete plans with the original empty layout cache and per-row terrain
/// batches. Restore the thread's setting on normal return and during unwinding.
#[cfg(test)]
pub(super) fn with_original_rows<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) { ORIGINAL_ROWS.with(|flag| flag.set(self.0)); }
    }
    let _restore = Restore(ORIGINAL_ROWS.with(|flag| flag.replace(true)));
    crate::water::with_original_batches(f)
}

/// Optional CPU measurements, only while the isolated, serial release budget replans
/// a known cell. Failure deltas use the census's process-global counters.
#[cfg(test)]
pub(super) fn profile(enabled: bool) {
    PROFILE.with(|p| p.set(enabled));
    crate::water::profile_queries(enabled);
}

#[cfg(test)]
fn phase_start() -> Option<std::time::Duration> {
    PROFILE.with(|p| p.get()).then(crate::perf::thread_time)
}

#[cfg(test)]
fn phase_end(name: &str, start: Option<std::time::Duration>) {
    if let Some(start) = start {
        eprintln!("  plan phase {name}: {:.3} ms CPU", (crate::perf::thread_time() - start).as_secs_f64() * 1000.0);
    }
}

#[cfg(test)]
fn report_close_scratch(water: &crate::water::Freshwater) {
    if PROFILE.with(|p| p.get()) {
        let scratch = water.work();
        eprintln!("  close scratch: corner clears {}, raw-column clears {}, peak corners {}, peak raw columns {}", scratch[0], scratch[1], scratch[2], scratch[3]);
    }
}

fn miss<T>(reason: usize) -> Option<T> {
    #[cfg(test)]
    PLACE_FAILS[reason].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = reason;
    None
}

/// Gives a plan up for `reason` (counted in tests).
fn give_up<T>(reason: usize) -> Option<T> {
    #[cfg(test)]
    GIVEN_UP[reason].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = reason;
    None
}

/// What the planner works with.
pub struct Ctx<'a> {
    pub world: &'a Overworld,
    pub land: &'a dyn Land,
    pub def: &'a SettlementsDef,
    pub resolved: &'a super::def::Resolved,
    pub fixed: &'a Fixed,
    pub seed: u64,
    /// Near where a new player spawns, in the square settled for them: it is always
    /// settled, within `spawn_settlement_within` of this.
    pub spawn: Option<IVec2>,
    /// The squares one of which may be settled for a new player whatever their roll (those
    /// in reach of where they spawn), whose names their neighbours keep off too.
    pub promised: &'a [IVec2],
    /// How well the families suit region cells, remembered (see `Settlements::cells`).
    pub cells: &'a crate::water::cache::Cache<IVec2, super::culture::Fits>,
}

/// Picks from `items` by weight; `None` when every weight is nothing.
pub(super) fn pick<T: Copy>(rng: &mut Rng, items: &[(T, f32)]) -> Option<T> {
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

/// Whether ground can take a lane, or be the middle of a settlement: solid, no crag, and
/// out of rivers' and lakes' valleys.
fn firm(g: &GeoSample) -> bool {
    !g.submerged() && g.river < 0.08 && g.crag.amplitude <= 0.0 && solid(g)
}

/// Whether ground can take a lane or a prop: firm, but for the river valley's floor short
/// of its channel.
fn walkable(g: &GeoSample) -> bool {
    !g.submerged() && g.river < 0.5 && g.crag.amplitude <= 0.0 && solid(g)
}

/// Whether ground is dry and solid at all, a bank included.
fn standing(g: &GeoSample) -> bool {
    !g.submerged() && solid(g)
}

/// Whether ground can take a building: no terrain shape or cave under it (a crag is
/// graded away under its pad, so long as the ground is level enough).
fn solid(g: &GeoSample) -> bool {
    g.form.shape.is_none() && g.cavity.is_none()
}

const GROUND_SAMPLE_LIMIT: usize = 32_768;

/// The original terrain facts leveling reads; full samples stay in Ground's cache.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LevelSample {
    height: i32,
    river: f32,
    submerged: bool,
    solid: bool,
}

impl LevelSample {
    fn of(g: &GeoSample) -> Self {
        Self { height: g.height, river: g.river, submerged: g.submerged(), solid: solid(g) }
    }
}

/// The ground, sampled once per position.
struct Ground<'a> {
    land: &'a dyn Land,
    seen: FxHashMap<IVec2, GeoSample>,
    dry_water: rustc_hash::FxHashSet<IVec2>,
    scratch: crate::water::Samples,
    #[cfg(test)]
    queries: usize,
    #[cfg(test)]
    misses: usize,
    #[cfg(test)]
    clears: usize,
    #[cfg(test)]
    levels: usize,
    #[cfg(test)]
    prefetched: usize,
    /// Explicit full-sample contains/get reads in the prefetch and at controls.
    #[cfg(test)]
    cache_reads: usize,
}

impl Ground<'_> {
    fn new(land: &dyn Land) -> Ground<'_> {
        Ground { land, seen: FxHashMap::default(), dry_water: Default::default(), scratch: Default::default(), #[cfg(test)] queries: 0, #[cfg(test)] misses: 0, #[cfg(test)] clears: 0, #[cfg(test)] levels: 0, #[cfg(test)] prefetched: 0, #[cfg(test)] cache_reads: 0 }
    }

    /// Continue only the site search over this same land. Full sealed samples and
    /// false water witnesses stay distinct; the sampler's exact keys move intact.
    fn from_site(land: &dyn Land, water: crate::water::Freshwater) -> Ground<'_> {
        let mut ground = Self::new(land);
        (ground.seen, ground.dry_water, ground.scratch) = water.into_work();
        ground
    }

    fn reserve(&mut self, count: usize) {
        if self.seen.len() + self.dry_water.len() + count > GROUND_SAMPLE_LIMIT {
            self.seen.clear();
            self.dry_water.clear();
            #[cfg(test)]
            { self.clears += 1; }
        }
        // Large retry sets will otherwise copy the terrain through an intermediate
        // table. Small plans keep the original growth, and the final table remains
        // below the entry bound; any further growth is the map's original policy.
        let capacity = self.seen.capacity();
        let large = GROUND_SAMPLE_LIMIT / 8 * 7;
        if capacity >= 4096 && capacity < large && self.seen.len() + count > capacity {
            self.seen.reserve(large - self.seen.len());
        }
    }

    /// Group only missing original samples for the next bounded portion of a row.
    /// Lookahead may outlive an earlier rejected point; every read stays in the timer.
    #[cfg(test)]
    fn prefetch(&mut self, first: IVec2, count: usize) {
        assert!(count <= crate::water::SAMPLE_BATCH);
        let mut points = [IVec2::ZERO; crate::water::SAMPLE_BATCH];
        for (i, p) in points[..count].iter_mut().enumerate() { *p = first + IVec2::new(i as i32, 0); }
        self.prefetch_points(&points[..count]);
    }

    /// A bounded set of original points, also used by wholly consumed water rings.
    fn prefetch_points(&mut self, points: &[IVec2]) {
        assert!(points.len() <= crate::water::SAMPLE_BATCH);
        self.reserve(points.len());
        let mut missing = Vec::new();
        for p in points {
            let point = (p.x, p.y);
            #[cfg(test)]
            { self.cache_reads += 1; }
            if !self.seen.contains_key(p) && !missing.contains(&point) {
                if missing.is_empty() { missing.reserve_exact(points.len()); }
                missing.push(point);
            }
        }
        let points = missing;
        if points.is_empty() { return; }
        #[cfg(test)]
        { self.prefetched += points.len(); }
        let samples = self.land.samples_cached(&points, &mut self.scratch);
        for ((x, z), g) in points.into_iter().zip(samples) {
            let p = IVec2::new(x, z);
            self.dry_water.remove(&p);
            self.seen.insert(p, g);
        }
    }

    /// Consecutive row-major positions are distinct. Keep their actual warm or new
    /// samples in order, so consuming this bounded batch needs no cache readback.
    fn prefetch_unique(&mut self, points: &[IVec2]) -> [Option<LevelSample>; crate::water::SAMPLE_BATCH] {
        assert!(points.len() <= crate::water::SAMPLE_BATCH);
        self.reserve(points.len());
        let mut values = [None; crate::water::SAMPLE_BATCH];
        let mut missing = [(0, 0); crate::water::SAMPLE_BATCH];
        let mut indices = [0; crate::water::SAMPLE_BATCH];
        let mut count = 0;
        for (i, &p) in points.iter().enumerate() {
            #[cfg(test)]
            { self.cache_reads += 1; }
            if let Some(g) = self.seen.get(&p) {
                values[i] = Some(LevelSample::of(g));
            } else {
                missing[count] = (p.x, p.y);
                indices[count] = i;
                count += 1;
            }
        }
        if count == 0 { return values; }
        #[cfg(test)]
        { self.prefetched += count; }
        let samples = self.land.samples_cached(&missing[..count], &mut self.scratch);
        for ((&(x, z), &i), g) in missing[..count].iter().zip(&indices[..count]).zip(samples) {
            let p = IVec2::new(x, z);
            self.dry_water.remove(&p);
            self.seen.insert(p, g);
            values[i] = Some(LevelSample::of(&g));
        }
        values
    }

    fn at(&mut self, p: IVec2) -> GeoSample {
        #[cfg(test)]
        { self.queries += 1; self.cache_reads += 1; }
        if let Some(g) = self.seen.get(&p) {
            return *g;
        }
        // Even a spawn cell's many layout retries keep bounded temporary storage.
        // Clearing changes only how often the original land is recomputed.
        self.reserve(1);
        #[cfg(test)]
        { self.misses += 1; }
        let g = self.land.sample_cached(p.x, p.y, &mut self.scratch);
        self.dry_water.remove(&p);
        self.seen.insert(p, g);
        g
    }

    /// The same Boolean house-water predicate, with exact negative witnesses kept
    /// separate from terrain. Full samples always take precedence; possible water
    /// still uses at(), including its original sealing and cache accounting.
    fn freshwater(&mut self, p: IVec2) -> bool {
        if let Some(g) = self.seen.get(&p) {
            #[cfg(test)]
            { self.queries += 1; }
            return crate::water::freshwater(g);
        }
        if self.dry_water.contains(&p) { return false; }
        if self.land.freshwater_absent_cached(p, &mut self.scratch) {
            self.reserve(1);
            self.dry_water.insert(p);
            return false;
        }
        crate::water::freshwater(&self.at(p))
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

    /// How far `p` is outside the blocks, or how deep inside them (negative).
    fn signed(&self, p: Vec2) -> f32 {
        let lo = self.min.as_vec2() - 0.5;
        let hi = self.max.as_vec2() + 0.5;
        let d = (lo - p).max(p - hi);
        if d.max_element() > 0.0 { d.max(Vec2::ZERO).length() } else { d.max_element() }
    }

    pub(super) fn grow(&self, by: i32) -> Rect {
        Rect { min: self.min - by, max: self.max + by }
    }

    fn spanning(a: IVec2, b: IVec2) -> Rect {
        Rect { min: a.min(b), max: a.max(b) }
    }

    /// Whether `p` lies inside.
    fn holds(&self, p: IVec2) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    /// Whether `other` lies wholly inside.
    fn covers(&self, other: &Rect) -> bool {
        self.holds(other.min) && self.holds(other.max)
    }
}

/// How far past a wall the lanes leading out of a walled place run, so the trails that
/// join them meet them clear of it: a trail may cross anything built in the cells of its
/// 8-block grid next to its end's, up to 15 blocks off.
const GATE_ROAD: f32 = 17.0;

/// A lane: its centre line and half its width.
#[derive(Clone, Debug)]
pub struct Lane {
    pub points: Vec<Vec2>,
    pub half: f32,
    pub paved: bool,
    /// Whether it leads out of the settlement, a trail joining its end.
    pub main: bool,
    /// Exact interpolation samples, shared by temporary lane clones. Plans retain only
    /// `points`; oversized lanes use the original calculation instead.
    samples: Option<Arc<LaneSamples>>,
}

#[derive(Debug)]
struct LaneSamples {
    points: Vec<Vec2>,
    bounds: Vec<(Vec2, Vec2)>,
    whole: (Vec2, Vec2),
}

const LANE_CHUNK: usize = 16;
const LANE_SAMPLE_LIMIT: usize = 16_384;

impl Lane {
    /// Drawing-only paths do not need rectangle-clearance samples.
    fn bare(points: Vec<Vec2>, half: f32, paved: bool, main: bool) -> Self {
        Self { points, half, paved, main, samples: None }
    }

    fn new(points: Vec<Vec2>, half: f32, paved: bool, main: bool) -> Self {
        let count = points.windows(2).try_fold(0usize, |total, w| {
            let n = (w[0].distance(w[1]).ceil() as i32).max(1) as usize;
            total.checked_add(n + 1).filter(|&count| count <= LANE_SAMPLE_LIMIT)
        });
        let samples = count.map(|count| {
            let mut sampled = Vec::with_capacity(count);
            for w in points.windows(2) {
                let n = (w[0].distance(w[1]).ceil() as i32).max(1);
                sampled.extend((0..=n).map(|i| w[0].lerp(w[1], i as f32 / n as f32)));
            }
            let bounds: Vec<_> = sampled.chunks(LANE_CHUNK).map(|chunk| {
                chunk.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(min, max), &p| (min.min(p), max.max(p)))
            }).collect();
            let whole = bounds.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(min, max), &(lo, hi)| (min.min(lo), max.max(hi)));
            Arc::new(LaneSamples { points: sampled, bounds, whole })
        });
        Self { points, half, paved, main, samples }
    }

    fn distance(&self, p: Vec2) -> f32 {
        self.points.windows(2).map(|w| segment_distance(p, w[0], w[1])).fold(f32::MAX, f32::min).min(self.points.first().map_or(f32::MAX, |q| q.distance(p)))
    }

    /// Clip paving's row-major scan to a conservative box. The projected segment
    /// point is `a + (b - a) * t`, with t in [0, 1]: rounded multiplication and
    /// addition are monotone, so its extrema are a and the *computed* a + (b - a),
    /// which need not equal b. Each axis's distance to that box is a lower bound
    /// under the same nonnegative square/sqrt operations as distance(). Binary
    /// searches keep all integer coordinates whose lower bound can pass, including
    /// integer-to-float rounding at large coordinates; no float-to-integer padding
    /// assumption is needed. Unusual non-finite paths keep the full original scan.
    fn paving_bounds(&self, raster: Rect) -> Option<Rect> {
        let Some(&first) = self.points.first() else { return Some(raster) };
        // distance() caps even overflowing distances at its MAX fold sentinel.
        if !first.is_finite() || !self.half.is_finite() || self.half < 0.0 || self.half == f32::MAX { return Some(raster); }
        let (mut min, mut max) = (first, first);
        for w in self.points.windows(2) {
            let ab = w[1] - w[0];
            let end = w[0] + ab;
            if !w[0].is_finite() || !ab.is_finite() || !end.is_finite() { return Some(raster); }
            min = min.min(w[0]).min(end);
            max = max.max(w[0]).max(end);
        }
        // First true value of a monotone predicate, or hi + 1. i64 also permits
        // the exclusive end when an original raster reaches i32::MAX.
        fn first_true(lo: i32, hi: i32, test: impl Fn(i32) -> bool) -> i64 {
            let (mut lo, mut end) = (i64::from(lo), i64::from(hi) + 1);
            while lo < end {
                let mid = lo + (end - lo) / 2;
                if test(mid as i32) { end = mid; } else { lo = mid + 1; }
            }
            lo
        }
        let axis = |lo, hi, min: f32, max: f32| {
            let left = first_true(lo, hi, |i| Vec2::new((min - i as f32).max(0.0), 0.0).length() <= self.half);
            let right = first_true(lo, hi, |i| Vec2::new((i as f32 - max).max(0.0), 0.0).length() > self.half) - 1;
            (left <= right).then_some((left as i32, right as i32))
        };
        let (lx, hx) = axis(raster.min.x, raster.max.x, min.x, max.x)?;
        let (lz, hz) = axis(raster.min.y, raster.max.y, min.y, max.y)?;
        Some(Rect { min: IVec2::new(lx, lz), max: IVec2::new(hx, hz) })
    }

    #[cfg(test)]
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

    /// Whether any sampled lane point passes a monotone distance predicate (`< reach`
    /// or `<= reach`). Chunk bounds use extrema of the actual interpolated floats. Each
    /// component of their distance is no greater than a contained point's component;
    /// the same nonnegative squares, sum and sqrt preserve that lower bound, including
    /// equality. A rejected bound therefore cannot hide a matching point.
    fn rect_any(&self, rect: &Rect, test: impl Fn(f32) -> bool) -> bool {
        #[cfg(test)]
        let profiling = PROFILE.with(|p| p.get());
        #[cfg(test)]
        let mut evaluations = 0;
        #[cfg(test)]
        let mut skips = 0;
        #[cfg(test)]
        let mut bounds_tested = 0;
        let mut point = |p| {
            #[cfg(test)]
            if profiling { evaluations += 1; }
            test(rect.distance(p))
        };
        let found = if let Some(samples) = &self.samples {
            let lo = rect.min.as_vec2() - 0.5;
            let hi = rect.max.as_vec2() + 0.5;
            let lower = |(min, max): (Vec2, Vec2)| (lo - max).max(min - hi).max(Vec2::ZERO).length();
            #[cfg(test)]
            if profiling { bounds_tested += 1; }
            if !test(lower(samples.whole)) {
                #[cfg(test)]
                if profiling { skips += 1; }
                false
            } else { samples.points.chunks(LANE_CHUNK).zip(&samples.bounds).any(|(chunk, &bounds)| {
                #[cfg(test)]
                if profiling { bounds_tested += 1; }
                if !test(lower(bounds)) {
                    #[cfg(test)]
                    if profiling { skips += 1; }
                    return false;
                }
                chunk.iter().copied().any(&mut point)
            }) }
        } else {
            self.points.windows(2).any(|w| {
                let n = (w[0].distance(w[1]).ceil() as i32).max(1);
                (0..=n).any(|i| point(w[0].lerp(w[1], i as f32 / n as f32)))
            })
        };
        #[cfg(test)]
        if profiling {
            let counts = [1, evaluations, skips, bounds_tested];
            LANE_COUNTS.with(|c| c.set(std::array::from_fn(|i| c.get()[i] + counts[i])));
        }
        found
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
/// The ground round the middle (`firm`) must be dry; water further out, within `water`,
/// is wanted (and required where `water` is above nothing), and buildings keep off it as
/// they are placed.
/// In the square a new player spawns in (`spawn`), only spots within the promised reach of
/// them are looked at, their climate rules none out, and several are kept, best first.
#[cfg(test)]
pub(super) fn site(ctx: &Ctx, cell: IVec2, spawn: Option<IVec2>) -> Vec<(IVec2, bool)> {
    site_with_water(ctx, cell, spawn, &mut crate::water::Freshwater::default())
}

/// Keep this call's exact terrain and close-water work for its subsequent layouts.
fn site_with_water(ctx: &Ctx, cell: IVec2, spawn: Option<IVec2>, water: &mut crate::water::Freshwater) -> Vec<(IVec2, bool)> {
    let (grid, site) = (&ctx.def.grid, &ctx.def.site);
    let step = site.step;
    let first = cell * grid.spacing + grid.margin;
    let size = (grid.spacing - 2 * grid.margin) / step + 1;
    #[cfg(test)]
    let time = phase_start();
    let samples = ctx.land.grid(first, step, size);
    #[cfg(test)]
    phase_end(&format!("site grid ({} points)", size * size), time);
    let at = |i: i32, j: i32| -> Option<&GeoSample> { (i >= 0 && j >= 0 && i < size && j < size).then(|| &samples[(j * size + i) as usize]) };
    let (reach, core, wet) = (site.reach / step, site.firm / step, site.water / step);
    let within = |di: i32, dj: i32, r: i32| di * di + dj * dj <= r * r + 1;
    let fresh_at = crate::water::freshwater;
    let mut candidates: Vec<(f32, IVec2, bool)> = Vec::new();
    for j in (reach..size - reach).step_by(2) {
        for i in (reach..size - reach).step_by(2) {
            let Some(centre) = at(i, j) else { continue };
            let mut heights = Vec::with_capacity(32);
            let mut bad = false;
            for dj in -reach..=reach {
                for di in -reach..=reach {
                    if !within(di, dj, reach) {
                        continue;
                    }
                    let Some(g) = at(i + di, j + dj) else { continue };
                    if !firm(g) {
                        bad |= within(di, dj, core) || g.submerged() && g.body == crate::Body::Sea;
                        continue;
                    }
                    heights.push(g.height);
                }
            }
            if bad || heights.is_empty() {
                continue;
            }
            let mean = heights.iter().sum::<i32>() as f32 / heights.len() as f32;
            let range = heights.iter().max().unwrap() - heights.iter().min().unwrap();
            if mean < (SEA_LEVEL + site.height.0) as f32 || mean > (SEA_LEVEL + site.height.1) as f32 || range > site.relief {
                continue;
            }
            let spread = (heights.iter().map(|&h| (h as f32 - mean).powi(2)).sum::<f32>() / heights.len() as f32).sqrt();
            // Fresh water near: a river or lake within `water`, as far as the samples tell
            // (a creek may slip between them, which the closer look below finds).
            let fresh = (-wet..=wet).any(|dj| (-wet..=wet).any(|di| within(di, dj, wet) && at(i + di, j + dj).is_some_and(fresh_at)));
            let luck = unit(hash3(ctx.seed ^ 0x517e, first.x + i * step, 0, first.y + j * step));
            let s = &site.score;
            let score = luck * s.luck - spread * s.spread - range as f32 * s.relief + if fresh { s.water } else { 0.0 } - (centre.height - SEA_LEVEL) as f32 * s.height;
            candidates.push((score, first + IVec2::new(i, j) * step, fresh));
        }
    }
    if let Some(spawn) = spawn {
        let within = ctx.def.rules.spawn_settlement_within as f32;
        candidates.retain(|c| c.1.as_vec2().distance(spawn.as_vec2()) <= within);
    }
    // The spawn's square looks further down its list, and keeps every spot that will do
    // so the planner may try the next where one cannot be laid out.
    let spawn_tries = ctx.def.rules.spawn_tries as usize;
    let tries = if spawn.is_some() { site.tries * spawn_tries } else { site.tries };
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then((a.1.x, a.1.y).cmp(&(b.1.x, b.1.y))));
    // Where water is required, a closer look round the best spots finds the creeks the
    // samples missed; the first `tries` with water near go on.
    if candidates.is_empty() {
        give_up::<()>(10);
        return Vec::new();
    }
    if site.water > 0 { ctx.land.remember_freshwater_grid(first, step, size, &samples, water); }
    let candidates = candidates
        .into_iter()
        .take(if site.water > 0 { tries.saturating_mul(site.look) } else { tries })
        .filter_map(|(_, at, fresh)| {
            if site.water > 0 {
                let near = fresh || ctx.land.freshwater_near(at, site.water, site.creek, water);
                near.then_some((at, true))
            } else {
                Some((at, fresh))
            }
        })
        // Keep the qualifying-prefix limit, but examine it lazily: a normal cell
        // returns its first usable site, and a spawn cell needs only `spawn_tries`.
        .take(tries);
    let mut found = Vec::new();
    let mut examined = 0;
    for (at, fresh) in candidates {
        examined += 1;
        let ground_site = ctx.world.site(at.x, at.y);
        if !habitable(ctx, &ground_site) {
            continue;
        }
        // Not beside a sinkhole or a ravine, which would open under its buildings.
        if ctx.world.caves.opens_near((at.x, at.y), site.caves) {
            continue;
        }
        // Fewer where it is cold or hot: as likely as anywhere in mild land, a `taper` of
        // that at the edges, none past them; and fewer in harsher biomes (`levels`). The
        // roll is the square's, made once on its best site: a miss leaves the square empty
        // rather than trying a milder spot, which would lift the rate in cold and hot land
        // above `taper`.
        let climate = site.climate(ground_site.temperature) * site.level(ctx.world.biomes().get(ground_site.home).level);
        if spawn.is_none() {
            if found.is_empty() && unit(hash3(ctx.seed ^ 0xc11a, cell.x, 0, cell.y)) >= climate {
                #[cfg(test)]
                report_close_scratch(water);
                give_up::<()>(2);
                return Vec::new();
            }
            // The best, then `others` more in turn should it not lay out.
            found.push((at, fresh));
            if found.len() > site.others {
                break;
            }
            continue;
        }
        found.push((at, fresh));
        if found.len() >= spawn_tries {
            break;
        }
    }
    if found.is_empty() {
        give_up::<()>(if examined == 0 { 0 } else { 1 });
    }
    #[cfg(test)]
    report_close_scratch(water);
    found
}

/// Whether a settlement may stand on this original site: in no biome it avoids, and
/// within its temperatures. The same full site also supplies fertility and home level.
fn habitable(ctx: &Ctx, site: &crate::Site) -> bool {
    let biome = ctx.world.biomes().get(site.biome);
    let (cold, hot) = ctx.def.site.temperature;
    !ctx.def.avoid.iter().any(|t| biome.has_tag(t)) && (cold..=hot).contains(&site.temperature)
}

/// The character of a settlement at `at`, each lever mixed of luck and its site as
/// `levers` in settlements.ron says: by default wealth from fertile soil, fresh water and
/// gentle ground, age from luck alone, order and crowding following wealth a little, and
/// green from the plants around.
fn levers(ctx: &Ctx, site: &crate::Site, fresh: bool, rng: &mut Rng) -> Levers {
    let def = &ctx.def.rules.levers;
    let flat = 1.0 - (site.steepness / def.steep).min(1.0);
    let forest = site.forest.max(site.vegetation * def.undergrowth);
    let mut l = [0.0; 5];
    for (lever, mix) in [(Lever::Wealth, &def.wealth), (Lever::Age, &def.age), (Lever::Order, &def.order), (Lever::Crowding, &def.crowding), (Lever::Green, &def.green)] {
        let rolls = mix.rolls.max(1);
        let luck = (0..rolls).map(|_| rng.unit()).sum::<f32>() / rolls as f32;
        let water = if fresh { 1.0 } else { 0.0 };
        let wealth = l[Lever::Wealth as usize];
        l[lever as usize] = (mix.luck * luck + mix.fertility * site.fertility + mix.water * water + mix.flat * flat + mix.wealth * wealth + mix.forest * forest).clamp(0.0, 1.0);
    }
    Levers { levers: l, axes: [0.0; super::culture::AXES] }
}

/// The materials a settlement keeps of its style: a few of each list, by weight.
struct Kit {
    wood: String,
    log: String,
    foundation: Palette,
    floor: Vec<(BlockId, f32)>,
    walls: Vec<(BlockId, f32)>,
    frame: Vec<(BlockId, f32)>,
    roof: Vec<(BlockId, f32)>,
    trims: Vec<(BlockId, f32)>,
    paving: Palette,
    hedge: Palette,
    weathered: Palette,
    chimney: Vec<(BlockId, f32)>,
    crops: Vec<(Vec<BlockId>, f32)>,
    fruit: Palette,
    roofs: Vec<(Roof, f32)>,
    window: Option<BlockId>,
    roof_pieces: Vec<(super::roofs::RoofPieces, f32)>,
    pieced: f32,
    jetty: Option<([BlockId; 4], f32)>,
}

fn kit(ctx: &Ctx, style: &StyleDef, levers: &Levers, culture: &Culture, crop_level: u32, rng: &mut Rng) -> Kit {
    let woods: Vec<(usize, f32)> = style.woods.iter().enumerate().map(|(i, (_, w))| (i, *w)).collect();
    let wood = style.woods[pick(rng, &woods).unwrap_or(0)].0.clone();
    let trim = if rng.chance(0.55) { wood.clone() } else { style.woods[pick(rng, &woods).unwrap_or(0)].0.clone() };
    let resolve = |list: &[(String, f32)]| -> Vec<(BlockId, f32)> {
        list.iter().filter_map(|(name, w)| ctx.resolved.get(&fill(name, &wood, &trim)).map(|id| (id, *w))).filter(|(_, w)| *w > 0.0).collect()
    };
    // The family's palette joins the style's, as `palette` times the style's weight.
    let palette = &culture.palette;
    let share = ctx.def.cultures.rules.palette;
    let join = |mut base: Vec<(BlockId, f32)>, extra: &super::def::Material| -> Vec<(BlockId, f32)> {
        let total: f32 = base.iter().map(|b| b.1).sum();
        let add = resolve(&extra.0);
        let added: f32 = add.iter().map(|b| b.1).sum();
        if added > 0.0 {
            let scale = total.max(1.0) * share / added;
            base.extend(add.into_iter().map(|(b, w)| (b, w * scale)));
        }
        base
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
    // Only crops of the size's level or below: a field is no shortcut past Farming.
    let crops = style
        .crops
        .iter()
        .filter(|(crop, w)| *w > 0.0 && ctx.def.crops.get(crop).is_none_or(|c| c.level <= crop_level))
        .filter_map(|(crop, w)| Some((ctx.def.stages(crop).iter().map(|s| ctx.resolved.get(s)).collect::<Option<Vec<_>>>()?, *w)))
        .filter(|(stages, _)| stages.len() >= 2)
        .collect();
    Kit {
        foundation: Palette(resolve(&style.foundation.0)),
        floor: keep(rng, resolve(&style.floor.0)),
        walls: keep(rng, join(resolve(&style.walls.0), &palette.walls)),
        frame: keep(rng, resolve(&style.frame.0)),
        roof: keep(rng, join(resolve(&style.roof.0), &palette.roof)),
        trims: keep(rng, join(resolve(&style.trim.0), &palette.trim)),
        paving: Palette(resolve(&style.paving.0)),
        hedge: Palette(resolve(&style.hedge.0)),
        weathered: Palette(resolve(&style.weathered.0)),
        chimney: resolve(&style.chimney.0),
        crops,
        fruit: Palette(ctx.fixed.fruit.iter().filter(|f| f.2 <= crop_level).map(|&(b, w, _)| (b, w)).collect()),
        // The style's roofs, each weighed again by how the place leans.
        roofs: style
            .roofs
            .iter()
            .map(|&(shape, w)| (shape, w * culture.mult(Listing::Roof, &format!("{shape:?}")) * ctx.def.roofs.iter().find(|r| r.shape == shape).map_or(1.0, |r| r.lean.weigh(r.weight, levers))))
            .collect(),
        window: style.window.as_deref().and_then(|w| ctx.resolved.get(w)),
        roof_pieces: style
            .roof_pieces
            .iter()
            .filter(|p| p.weight > 0.0)
            .filter_map(|p| Some((super::roofs::RoofPieces::resolve(p, &wood, &trim, |n| ctx.resolved.get(n))?, p.weight)))
            .collect(),
        pieced: style.pieced,
        // More jettied where the place is crowded, its street narrow.
        jetty: style.jetty.as_ref().and_then(|j| {
            let name = fill(&j.beam, &wood, &trim);
            let mut ids = [BlockId::AIR; 4];
            for (i, suffix) in super::roofs::SUFFIXES.iter().enumerate() {
                ids[i] = ctx.resolved.get(&format!("{name}{suffix}"))?;
            }
            Some((ids, (j.chance * (0.5 + levers.get(Lever::Crowding))).min(1.0)))
        }),
        log: style.log.clone(),
        wood,
    }
}

impl Kit {
    /// One building's materials, from those kept.
    fn mats(&self, ctx: &Ctx, rng: &mut Rng) -> Mats {
        let one = |rng: &mut Rng, list: &[(BlockId, f32)], or: BlockId| pick(rng, list).unwrap_or(or);
        let fallback = self.foundation.0.first().map_or(ctx.fixed.dirt, |b| b.0);
        let wall = one(rng, &self.walls, fallback);
        let log = ctx.resolved.get(&fill(&self.log, &self.wood, &self.wood)).unwrap_or(fallback);
        let chimney = one(rng, &self.chimney, fallback);
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
            chimney,
            crops: self.crops.clone(),
            fruit: self.fruit.clone(),
            roof_pieces: self.roof_pieces.clone(),
            pieced: self.pieced,
            jetty: self.jetty,
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
/// A block column of a place's wall, and what part of it.
#[derive(Clone, Copy, Debug)]
struct Walling {
    at: IVec2,
    part: WallPart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WallPart {
    Wall,
    /// A block higher, every other block along a stone wall.
    Merlon,
    /// Either side of a gate, a block higher.
    Post,
    /// Over a gate, three blocks clear above the lane.
    Lintel,
    Tower,
    /// A tower's corner, a block higher.
    TowerCorner,
}

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
    // The square settled for a new player always is, near enough to them.
    let spawn = ctx.spawn;
    if !rng.chance(def.grid.chance) && spawn.is_none() {
        return None;
    }
    // Where one spot cannot be laid out (too few homes fit, no level ground for the well),
    // the spawn's square tries its next; any other square has the one.
    #[cfg(test)]
    let time = phase_start();
    let mut water = crate::water::Freshwater::default();
    let sites = site_with_water(ctx, cell, spawn, &mut water);
    #[cfg(test)]
    {
        phase_end("site search total", time);
        if let Some(work) = crate::water::query_work() {
            eprintln!("  close water: {} queries, {} square-grid centres, {} coordinates visited, {} centres sampled, {} batches, {} unused lookahead samples, {} cached hits, {} cache resets", work[0], work[1], work[2], work[3], work[4], work[5], work[6], work[7]);
        }
    }
    // Site search and layout retries inspect overlapping land. Keep this call's exact
    // samples and relief/channel work, including the site's coarse and close looks.
    #[cfg(test)]
    let original = ORIGINAL_ROWS.with(|flag| flag.get());
    #[cfg(not(test))]
    let original = false;
    let mut ground = if original { Ground::new(ctx.land) } else { Ground::from_site(ctx.land, water) };
    let first = sites.iter().find_map(|&(centre, fresh)| lay_out(ctx, cell, centre, fresh, rng.clone(), false, &mut ground));
    if spawn.is_none() || first.is_some() {
        return first;
    }
    // The spawn's square tries each spot with other draws (another square, other lanes,
    // the well elsewhere), then as the smallest place, before it is left empty: only the
    // land may keep a new player from a settlement.
    let salted = |salt: u64| Rng::new(ctx.seed ^ mix(salt), 0x5e771e, cell.x, cell.y);
    let salts = u64::from(ctx.def.rules.spawn_salts);
    sites
        .iter()
        .find_map(|&(centre, fresh)| (1..salts).find_map(|salt| lay_out(ctx, cell, centre, fresh, salted(salt), false, &mut ground)))
        .or_else(|| sites.iter().find_map(|&(centre, fresh)| (0..salts).find_map(|salt| lay_out(ctx, cell, centre, fresh, salted(salt), true, &mut ground))))
}

/// The stream names are drawn from, apart from the rest of the plan so a square's
/// neighbours can draw the names it may take without planning it.
const NAMING: u64 = 0x4a3e_5eed;

/// The stream a place's wall draws from, apart from the rest so an unwalled place is laid
/// out as it would be anyway.
const WALLS: u64 = 0x3a11_5eed;

/// How many names each square offers to take; its neighbours keep off all of them.
pub(super) const NAME_OFFERS: u32 = 3;

/// The `k`th name square `cell` may take from names list `list`: a first part and a last
/// (never "Brookbrook"), or a hamlet's first part and a small word.
pub(super) fn offered_name(def: &SettlementsDef, seed: u64, cell: IVec2, list: usize, k: u32, small: bool) -> (String, String) {
    let names = &def.cultures.families[list].names;
    let (first, last) = offered_parts(def, seed, cell, list, k, small);
    if small { return small_name(names.script, first, last); }
    (format!("{}{}", first.0, last.0), join_parts(&def.kana, names.script, first, last))
}

/// Draw the same name parts without constructing an unused reading for a neighbour.
fn offered_parts(def: &SettlementsDef, seed: u64, cell: IVec2, list: usize, k: u32, small: bool) -> (&(String, String), &(String, String)) {
    let names = &def.cultures.families[list].names;
    let mut rng = Rng::new(seed, NAMING + 1 + (list as u64) * 1024 + k as u64, cell.x, cell.y);
    let first = &names.first[rng.below(names.first.len() as u32) as usize];
    if small {
        let word = &names.small[rng.below(names.small.len() as u32) as usize];
        return (first, word);
    }
    let mut last = &names.last[rng.below(names.last.len() as u32) as usize];
    for _ in 0..4 {
        if !echoes(first, last) {
            break;
        }
        last = &names.last[rng.below(names.last.len() as u32) as usize];
    }
    (first, last)
}

fn offered_english(def: &SettlementsDef, seed: u64, cell: IVec2, list: usize, k: u32, small: bool) -> String {
    let (first, last) = offered_parts(def, seed, cell, list, k, small);
    if small { format!("{} {}", first.0, last.0) } else { format!("{}{}", first.0, last.0) }
}

/// A hamlet's name of a first part and a word: "Ash Well", アッシュ・ウェル.
/// (Matsu Juku, 松宿 in kanji.)
fn small_name(script: Script, first: &(String, String), word: &(String, String)) -> (String, String) {
    let joiner = if script == Script::Kanji { "" } else { "・" };
    (format!("{} {}", first.0, word.0), format!("{}{joiner}{}", first.1, word.1))
}

/// A name's two parts joined in Japanese by the family's script.
fn join_parts(kana: &[KanaJoin], script: Script, first: &(String, String), last: &(String, String)) -> String {
    match script {
        Script::Kana => join_kana(kana, first, last),
        Script::Kanji => format!("{}{}", first.1, last.1),
    }
}

/// Whether one part holds the other ("Brook" and "brook"), which reads badly joined.
fn echoes(first: &(String, String), last: &(String, String)) -> bool {
    let (a, b) = (first.0.to_lowercase(), last.0.to_lowercase());
    a.contains(&b) || b.contains(&a)
}

/// A periodic colouring of settlement squares. Two equal colours differ by at
/// least `span` squares in one direction, beyond the closest possible site pair's
/// naming reach. Equal complete readings own one colour across all families.
pub(super) fn name_span(def: &SettlementsDef) -> i32 {
    def.rules.names_apart.div_euclid(def.grid.spacing) + 2
}

pub(super) fn name_owner(reading: &str, span: i32) -> usize {
    let hash = reading.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    (mix(hash ^ (NAMING + 1)) % (span as u64 * span as u64)) as usize
}

/// Validate that every family can name every colour and that equal English names
/// with different readings still have the same owner. This also catches aliases
/// across families; equal Japanese readings have equal owners by construction.
pub(super) fn check_name_ownership(def: &SettlementsDef) -> Result<(), crate::content::ContentError> {
    if def.rules.names_apart == 0 { return Ok(()); }
    let span = name_span(def);
    let mut english = std::collections::BTreeMap::new();
    let mut japanese = std::collections::BTreeMap::new();
    let mut alias = |name: (String, String), owner| -> Result<(), crate::content::ContentError> {
        if english.insert(name.0.clone(), owner).is_some_and(|old| old != owner)
            || japanese.insert(name.1.clone(), owner).is_some_and(|old| old != owner)
        {
            return Err(crate::content::ContentError(format!("cultures.ron: name {:?} has aliases with different owners", name)));
        }
        Ok(())
    };
    for family in &def.cultures.families {
        let names = &family.names;
        let mut colours = vec![false; (span * span) as usize];
        for first in &names.first {
            // The offered stream can retain an echo after four retries. Include
            // every pair in alias validation; fallback coverage uses non-echoes.
            let full = names.last.iter().map(|last| ((format!("{}{}", first.0, last.0), join_parts(&def.kana, names.script, first, last)), !echoes(first, last)));
            let small = names.small.iter().map(|last| (small_name(names.script, first, last), false));
            for ((name, reading), normal) in full.chain(small) {
                let owner = name_owner(&reading, span);
                if normal { colours[owner] = true; }
                alias((name.clone(), reading.clone()), owner)?;
                if names.prefix_chance > 0.0 {
                    let joiner = if names.script == Script::Kanji { "" } else { "・" };
                    // Floating-point subtraction in pick() may fall through to
                    // the last entry even at zero weight. Validate every variant.
                    for prefix in &names.prefixes {
                        alias((format!("{} {name}", prefix.0), format!("{}{joiner}{reading}", prefix.1)), owner)?;
                    }
                }
            }
        }
        if colours.iter().any(|&present| !present) {
            return Err(crate::content::ContentError(format!("cultures.ron: family `{}` needs a name for each of {} naming colours", family.name, span * span)));
        }
    }
    Ok(())
}

/// Only families that can spell this literal English candidate can offer it.
/// Build each such family's original neighbour offers once, on first use. Echoes
/// remain eligible here: the offered stream can keep one after its four retries.
fn name_is_taken(def: &SettlementsDef, seed: u64, neighbours: &[IVec2], offered: &mut [Option<rustc_hash::FxHashSet<String>>], name: &str) -> bool {
    for (l, family) in def.cultures.families.iter().enumerate() {
        let names = &family.names;
        let can_spell = names.first.iter().any(|first| name.strip_prefix(&first.0).is_some_and(|end| {
            names.last.iter().any(|last| end == last.0)
                || end.strip_prefix(' ').is_some_and(|word| names.small.iter().any(|small| word == small.0))
        }));
        if !can_spell { continue; }
        let offers = offered[l].get_or_insert_with(|| {
            let mut offers = rustc_hash::FxHashSet::default();
            for &other in neighbours {
                for k in 0..NAME_OFFERS {
                    for small in [false, true] {
                        if small && names.small.is_empty() { continue; }
                        offers.insert(offered_english(def, seed, other, l, k, small));
                    }
                }
            }
            offers
        });
        if offers.contains(name) { return true; }
    }
    false
}

/// Each complete name belongs to a periodic cell colour, so nearby squares
/// cannot select the same name even past their three initial offers. Prefer its
/// own offers, then further draws, then every name, avoiding neighbours' offers
/// while its owned pool has others. Exhaustion can reuse an offered owned name:
/// that neighbour's different colour prevents it from selecting the name.
pub(super) fn unique_name(def: &SettlementsDef, seed: u64, promised: &[IVec2], cell: IVec2, list: usize, small: bool) -> (String, String) {
    let (grid, apart) = (&def.grid, def.rules.names_apart);
    let first = offered_name(def, seed, cell, list, 0, small);
    if apart <= 0 {
        return first;
    }
    let span = name_span(def);
    let colour = (cell.y.rem_euclid(span) * span + cell.x.rem_euclid(span)) as usize;
    let owns = |name: &(String, String)| name_owner(&name.1, span) == colour;
    let inner = |c: IVec2| (c * grid.spacing + grid.margin, (c + 1) * grid.spacing - grid.margin - 1);
    let (lo, hi) = inner(cell);
    let reach = apart.div_euclid(grid.spacing) + 1;
    let mut neighbours = Vec::new();
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let other = cell + IVec2::new(dx, dz);
            let (olo, ohi) = inner(other);
            let gap = (olo - hi).max(lo - ohi).max(IVec2::ZERO).as_vec2().length();
            // Squares too far, and those whose roll holds no settlement and that are not
            // the spawn's to settle (see `plan`).
            if other == cell || gap > apart as f32 || !(Rng::new(seed, 0x5e771e, other.x, other.y).chance(grid.chance) || promised.contains(&other)) {
                continue;
            }
            neighbours.push(other);
        }
    }
    // Its own offers first; a hamlet's small names are few (twenty first parts by three
    // words), so where those are taken it offers a first and last part instead, of which
    // there are hundreds (its neighbours keep off both forms). Then names no one offered.
    let offers = |small: bool, ks: std::ops::Range<u32>| ks.map(move |k| offered_name(def, seed, cell, list, k, small));
    let forms = if small { vec![true, false] } else { vec![false] };
    let own = forms.iter().flat_map(|&f| offers(f, 0..NAME_OFFERS));
    let further = forms.iter().flat_map(|&f| offers(f, NAME_OFFERS..NAME_OFFERS + 32));
    // Past those, every name the list makes, in order: a taken name only where the list
    // has none left (it makes hundreds; the neighbours offer a few each).
    let names = &def.cultures.families[list].names;
    let every = forms.iter().flat_map(move |&f| {
        names.first.iter().flat_map(move |a| {
            let words: Box<dyn Iterator<Item = (String, String)>> = if f {
                Box::new(names.small.iter().map(move |w| small_name(names.script, a, w)))
            } else {
                Box::new(names.last.iter().filter(move |b| !echoes(a, b)).map(move |b| (format!("{}{}", a.0, b.0), join_parts(&def.kana, names.script, a, b))))
            };
            words
        })
    });
    let mut offered = vec![None; def.cultures.families.len()];
    let mut first_owned = None;
    for name in own.chain(further).chain(every).filter(owns) {
        if !name_is_taken(def, seed, &neighbours, &mut offered, &name.0) { return name; }
        first_owned.get_or_insert(name);
    }
    // Even if every owned name is offered nearby, those other colours cannot
    // select it. Validated catalogues always supply at least one owned name.
    first_owned.expect("validated family has a name for every colour")
}

/// Lays out the settlement of the square `cell` round `centre`, if it can.
/// `smallest`: as the size with fewest houses, whatever the draw.
#[allow(clippy::too_many_arguments)]
fn lay_out(ctx: &Ctx, cell: IVec2, centre: IVec2, fresh: bool, rng: Rng, smallest: bool, ground: &mut Ground) -> Option<Plan> {
    #[cfg(test)]
    let time = phase_start();
    #[cfg(test)]
    let before = (ground.queries, ground.misses, ground.clears, ground.levels, LANE_COUNTS.with(|c| c.get()), ground.prefetched);
    #[cfg(test)]
    let failures = time.map(|_| (PLACE_FAILS.each_ref().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)), GIVEN_UP.each_ref().map(|c| c.load(std::sync::atomic::Ordering::Relaxed))));
    // A size too big for the ground lays out as its `smaller` (a town as a village), with
    // the same draws. Rows of houses change what else fits (frontage, plots, the homes
    // left to place): a place given up on after building one is laid out again, the same,
    // with no rows, so no place is lost to its rows.
    let mut drawn = None;
    let with_rows = |ground: &mut Ground, smallest: bool, instead: Option<usize>, drawn: &mut Option<usize>| {
        let mut rowed = false;
        let result = layout(ctx, cell, centre, fresh, rng.clone(), smallest, instead, drawn, true, &mut rowed, ground);
        if result.is_none() && rowed { layout(ctx, cell, centre, fresh, rng.clone(), smallest, instead, drawn, false, &mut rowed, ground) } else { result }
    };
    let result = with_rows(ground, smallest, None, &mut drawn);
    let smaller = drawn.and_then(|i| ctx.def.sizes[i].smaller.as_ref()).and_then(|name| ctx.def.sizes.iter().position(|s| s.name == *name));
    let result = match smaller {
        Some(smaller) if result.is_none() && !smallest => with_rows(ground, false, Some(smaller), &mut drawn),
        _ => result,
    };
    #[cfg(test)]
    if time.is_some() {
        let lane = LANE_COUNTS.with(|c| c.get());
        eprintln!("  layout {}: ground queries {}, hits {}, scalar misses {}, prefetched reads {}, total terrain reads {}, clears {}, level calls {}; lane checks {}, point distances {}, bound distances {}, chunks skipped {}",
            if result.is_some() { "accepted" } else { "rejected" }, ground.queries - before.0,
            (ground.queries - before.0) - (ground.misses - before.1), ground.misses - before.1,
            ground.prefetched - before.5, ground.misses - before.1 + ground.prefetched - before.5,
            ground.clears - before.2, ground.levels - before.3,
            lane[0] - before.4[0], lane[1] - before.4[1], lane[3] - before.4[3], lane[2] - before.4[2]);
        let scratch = ground.scratch.work();
        eprintln!("  scratch cumulative: corner clears {}, raw-column clears {}, peak corners {}, peak raw columns {}", scratch[0], scratch[1], scratch[2], scratch[3]);
        if let Some((places, abandoned)) = failures {
            let places = std::array::from_fn::<_, 8, _>(|i| PLACE_FAILS[i].load(std::sync::atomic::Ordering::Relaxed) - places[i]);
            let abandoned = std::array::from_fn::<_, 12, _>(|i| GIVEN_UP[i].load(std::sync::atomic::Ordering::Relaxed) - abandoned[i]);
            eprintln!("  layout placement failures [spec,bounds,occupied,lane,land,slope,road,unused]: {places:?}; give-up reasons: {abandoned:?}");
        }
        phase_end("layout total (including rejected attempts)", time);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn layout(ctx: &Ctx, cell: IVec2, centre: IVec2, fresh: bool, mut rng: Rng, smallest: bool, instead: Option<usize>, drawn: &mut Option<usize>, rows: bool, rowed: &mut bool, mut ground: &mut Ground) -> Option<Plan> {
    #[cfg(test)]
    let time = phase_start();
    let def = ctx.def;
    let ground_site = ctx.world.site(centre.x, centre.y);
    let mut levers = levers(ctx, &ground_site, fresh, &mut rng);
    let biome = ctx.world.biomes().get(ground_site.biome);
    // Whose it is: a family of the land round, its axes and its mutations, from streams
    // of their own so the rolls below are the ones they were.
    let probe = |p: IVec2| ctx.world.regional_biome(p.x, p.y);
    let cell_family = |g: IVec2| def.cultures.cached_family(ctx.seed, g, ctx.cells, &probe);
    let site = Site {
        seed: ctx.seed,
        cell,
        at: centre,
        level: ctx.world.biomes().get(ground_site.home).level as u32,
        fresh,
        relief: (ground_site.steepness / def.rules.levers.steep).min(1.0),
        forest: ground_site.forest,
        levers,
        cell_family: &cell_family,
    };
    let mut culture = def.cultures.roll(&site);
    levers.axes = culture.axes;
    let sizes: Vec<(usize, f32)> = def.sizes.iter().enumerate().map(|(i, s)| (i, s.lean.weigh(s.weight, &levers))).collect();
    let size_index = pick(&mut rng, &sizes)?;
    let size_index = if smallest { (0..def.sizes.len()).min_by_key(|&i| def.sizes[i].houses).unwrap_or(size_index) } else { size_index };
    *drawn = Some(size_index);
    let size_index = instead.unwrap_or(size_index);
    let size = &def.sizes[size_index];
    def.cultures.finish(&mut culture, &site, &size.name);
    levers.axes = culture.axes;
    // Styles weigh by how many of their tags the biome has, so where two fit (a cool
    // forest with conifers) either may be built; the catch-all style stands in temperate
    // and cool land, and wherever nothing else fits.
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
            (i, w * w * culture.mult(Listing::Style, &s.name))
        })
        .collect();
    let style_index = pick(&mut rng, &styles)?;
    let style = &def.styles[style_index];
    let layouts: Vec<(usize, f32)> = def.layouts.iter().enumerate().filter(|(_, l)| fresh || !l.water).map(|(i, l)| (i, l.lean.weigh(l.weight, &levers) * culture.mult(Listing::Layout, &l.name))).collect();
    let layout_index = pick(&mut rng, &layouts)?;
    let shape = def.layouts[layout_index].shape;
    let kit = kit(ctx, style, &levers, &culture, size.crop_level, &mut rng);
    #[cfg(test)]
    phase_end("site/levers/culture/kit", time);
    #[cfg(test)]
    let time = phase_start();
    let list = culture.family;
    let names = &def.cultures.families[list].names;
    let small_name = size_index == 0 && !names.small.is_empty() && Rng::new(ctx.seed, NAMING, cell.x, cell.y).chance(names.small_chance);
    let prefixes: Vec<(usize, f32)> = names.prefixes.iter().enumerate().map(|(i, p)| (i, p.3.weigh(p.2, &levers))).collect();
    // Consume both original draws here, even when the chance is false. Name strings
    // use an independent stream and are needed only after this layout succeeds.
    let prefix = (rng.chance(names.prefix_chance), pick(&mut rng, &prefixes));

    #[cfg(test)]
    phase_end("name choices", time);
    #[cfg(test)]
    let time = phase_start();
    let square_height = ground.at(centre).height;
    let order = levers.get(Lever::Order);
    let crowding = levers.get(Lever::Crowding);
    let wealth = levers.get(Lever::Wealth);
    let (rules, lanes_def) = (&def.rules, &def.rules.lanes);
    let mut square = match shape {
        Shape::Scatter => lanes_def.scatter,
        Shape::Green => (between(&mut rng, size.square) as f32 * lanes_def.green.0) as i32 + lanes_def.green.1,
        _ => between(&mut rng, size.square),
    };
    // The one great thing it is known by, if it has one: on the square, which grows to
    // hold it and the well beside it, or out at the end of a lane.
    let centres: Vec<(usize, f32)> = def.centrepieces.iter().enumerate().map(|(i, c)| (i, c.lean.weigh(c.weight, &levers) * culture.mult(Listing::Centrepiece, &c.kind.label()))).collect();
    let centrepiece = if rng.chance(size.centrepiece) { pick(&mut rng, &centres) } else { None };
    let mut centre_spec = centrepiece.map(|i| centre_spec(def, i, &kit, &levers, &mut rng, style));
    let mut on_square = centrepiece.is_some_and(|i| def.centrepieces[i].spot == Spot::Square);
    // A centrepiece on the square needs firm, even ground under it all, or it is left out.
    if on_square
        && let Some(spec) = &centre_spec
    {
        let extent = IVec2::new(spec.w, spec.d);
        let slope = rules.specials.get("centrepiece").map_or(3, |s| s.slope);
        if level(&mut ground, &Rect::new(centre - extent / 2, extent), 0, 2).is_none_or(|(low, high)| high - low > slope) {
            on_square = false;
            centre_spec = None;
        }
    }
    if on_square && let Some(spec) = &centre_spec {
        square = square.max(spec.w.max(spec.d) / 2 + lanes_def.centre.0).min(lanes_def.centre.1);
    }
    // Lanes lead off the square and never back across it.
    let square_rect = Rect { min: centre - square, max: centre + square };
    // Everything it builds stays inside its square of the grid, a margin in, so it never
    // reaches the next.
    let bounds = Rect { min: cell * def.grid.spacing + def.grid.margin / 2, max: (cell + 1) * def.grid.spacing - def.grid.margin / 2 - 1 };

    // Lanes.
    let mut lanes: Vec<Lane> = Vec::new();
    let lane_count = between(&mut rng, size.lanes);
    let main_half = size.lane_half;
    let side_half = lanes_def.side;
    let paved = wealth > size.paving.1;
    let wiggle = lanes_def.wiggle.0 + lanes_def.wiggle.1 * (1.0 - order);
    let start_angle = rng.unit() * std::f32::consts::TAU;
    let lane_length = |rng: &mut Rng| between(rng, size.lane_length) as f32;
    let walk_from = |ground: &mut Ground, lanes: &mut Vec<Lane>, rng: &mut Rng, from: Vec2, angle: f32, length: f32, half: f32, main: bool| {
        let points = walk(ground, lanes, rng, from, angle, length, wiggle, &bounds, lanes_def, &[(square_rect, CLEAR)]);
        if points.len() >= 3 {
            lanes.push(Lane::new(points, half, paved && main, main));
        }
    };
    let ring = square as f32 + 0.5;
    let from_square = |angle: f32| centre.as_vec2() + Vec2::from_angle(angle) * ring;
    // A straight lane, where the ground between its ends is firm.
    let straight = |ground: &mut Ground, from: Vec2, to: Vec2, half: f32| -> Option<Lane> {
        let n = (from.distance(to) / 2.0).ceil().max(1.0) as i32;
        (0..=n).all(|i| walkable(&ground.at(from.lerp(to, i as f32 / n as f32).round().as_ivec2()))).then(|| Lane::new(vec![from, to], half, false, false))
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
                    if rng.chance(lanes_def.branch) {
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
            let radius = square as f32 + lanes_def.ring.0 + lanes_def.ring.1 * (1.0 - crowding);
            let n = (radius * std::f32::consts::TAU / 3.0).ceil() as usize;
            let wobble = lanes_def.wobble * (1.0 - order);
            let mut points = Vec::with_capacity(n + 1);
            let mut whole = true;
            for k in 0..=n {
                let a = start_angle + k as f32 * std::f32::consts::TAU / n as f32;
                let r = radius + (unit(hash3(ctx.seed ^ 0x5149, cell.x, (k % n) as i32, cell.y)) - 0.5) * 2.0 * wobble;
                let p = centre.as_vec2() + Vec2::from_angle(a) * r;
                let g = ground.at(p.round().as_ivec2());
                if !walkable(&g) || (g.height - square_height).abs() > lanes_def.ring_rise || !bounds.holds(p.round().as_ivec2()) {
                    whole = false;
                    break;
                }
                points.push(p);
            }
            let reach = if whole {
                lanes.push(Lane::new(points, side_half + 0.2, false, false));
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
            let toward = water_near(&mut ground, centre, lanes_def.riverside);
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
            let step = lanes_def.terrace.0 + lanes_def.terrace.1 * (1.0 - crowding);
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
        return give_up(3);
    }
    #[cfg(test)]
    phase_end("square/lanes", time);
    #[cfg(test)]
    let lots_time = phase_start();

    // What to build: the size's own places, then homes and working buildings.
    let mut wanted: Vec<Special> = size.always.clone();
    for &(special, chance) in &size.sometimes {
        if rng.chance(chance) {
            wanted.push(special);
        }
    }
    // Every place keeps animals or grows food: a field where it has neither, or a pen
    // where it is not green.
    if !wanted.iter().any(|s| matches!(s, Special::Field | Special::Pen)) {
        wanted.push(if rng.chance(rules.lots.field.0 + rules.lots.field.1 * levers.get(Lever::Green)) { Special::Field } else { Special::Pen });
    }
    let homes = between(&mut rng, size.houses);
    let workshops = between(&mut rng, size.workshops);
    let mut lots: Vec<Lot> = Vec::new();
    let mut occupied: Vec<Rect> = Vec::new();
    // The well and centrepiece, which lanes keep a ring clear of.
    let mut kept: Vec<Rect> = Vec::new();
    let gap = rules.lots.gap.0 + ((1.0 - crowding) * rules.lots.gap.1).round() as i32;

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
        kept.push(rect);
        lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0, house: None, centre: centrepiece });
        let side = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y][rng.below(4) as usize];
        well_at = centre + side * (extent.max_element() / 2 + lanes_def.well.0);
    }
    // The well at the heart, or as near it as the ground is firm and level; a place that
    // must have one and has nowhere for it is not built.
    if wanted.contains(&Special::Well) {
        let width = between(&mut rng, size.well);
        let spec = special_spec(Kind::Well, width, width, &kit, &levers, &mut rng, style, rules);
        let extent = IVec2::splat(width);
        let spots = std::iter::once(well_at).chain((1..=square.max(2)).flat_map(|r| [IVec2::new(r, 0), IVec2::new(-r, 0), IVec2::new(0, r), IVec2::new(0, -r)].map(|d| well_at + d)));
        let mut placed = false;
        for at in spots {
            let rect = Rect::new(at - width / 2, extent);
            if occupied.iter().any(|o| o.gap(&rect) < 1) || lanes.iter().any(|l| l.rect_any(&rect.grow(RING), |d| d < l.half + 0.5)) {
                continue;
            }
            if level(&mut ground, &rect, def.site.bank, 1).is_none_or(|(low, high)| high - low > lanes_def.well.1 || (low - square_height).abs() > lanes_def.well.1) {
                continue;
            }
            let mats = kit.mats(ctx, &mut rng);
            let frame = Frame::fit(IVec2::ZERO, extent - 1, Facing::North, rect.min, square_height);
            occupied.push(rect);
            kept.push(rect);
            lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0, house: None, centre: None });
            placed = true;
            break;
        }
        if !placed {
            return give_up(4);
        }
    }
    // Stalls round the square, facing in, on firm ground.
    let stalls = wanted.iter().filter(|s| **s == Special::Stall).count();
    for k in 0..stalls {
        let angle = start_angle + 0.5 + k as f32 * std::f32::consts::TAU / stalls.max(1) as f32 + rng.unit() * 0.4;
        let p = centre.as_vec2() + Vec2::from_angle(angle) * (square as f32 - 2.0);
        let facing = Facing::toward(centre.as_vec2() - p);
        let spec = special_spec(Kind::Stall, 0, 0, &kit, &levers, &mut rng, style, rules);
        let (low, high) = spec.envelope();
        let extent = Frame::extent(low, high, facing);
        let rect = Rect::new(p.round().as_ivec2() - extent / 2, extent);
        if occupied.iter().any(|o| o.gap(&rect) < 1) || lanes.iter().any(|l| l.rect_any(&rect, |d| d < l.half + 0.5)) {
            continue;
        }
        let slope = rules.specials.get("stall").map_or(2, |s| s.slope);
        if level(&mut ground, &rect, def.site.bank, 1).is_none_or(|(low, high)| high - low > slope) {
            continue;
        }
        let mats = kit.mats(ctx, &mut rng);
        let frame = Frame::fit(low, high, facing, rect.min, square_height);
        occupied.push(rect);
        lots.push(Lot { spec, rect, frame, floor: square_height, mats, door: None, yard: 0, house: None, centre: None });
    }

    // The rest go along the lanes: grand places nearest the square, then the works it
    // always has, homes and working buildings mixed, and fields, pens, orchards and
    // woodyards out at the ends.
    let mut queue: Vec<Want> = Vec::new();
    for s in &wanted {
        match s {
            Special::Inn => queue.push(Want::Kind(Kind::Inn)),
            Special::Smithy => queue.push(Want::Kind(Kind::Smithy)),
            Special::Trader => queue.push(Want::Kind(Kind::Trader)),
            _ => {}
        }
    }
    queue.extend(size.works.iter().map(|w| Want::Work(w.clone())));
    let mut mixed: Vec<Want> = (0..homes).map(|_| Want::Home).chain((0..workshops).map(|_| Want::Workshop)).collect();
    for i in (1..mixed.len()).rev() {
        mixed.swap(i, rng.below(i as u32 + 1) as usize);
    }
    queue.extend(mixed);
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

    // These weights stay fixed throughout a layout. Availability is rebuilt after
    // each counted house; extras still draw in catalogue order for every attempt.
    let house_weights = def.houses.iter().map(|h| h.lean.weigh(h.weight, &levers) * culture.mult(Listing::House, &h.name)).collect();
    let extra_chances = def.extras.iter().map(|e| e.lean.weigh(e.chance, &levers) * culture.mult(Listing::Extra, &format!("{:?}", e.name))).collect();
    let mut town = Town { ctx, kit: &kit, levers: &levers, house_weights, extra_chances, style, size: &size.name, built: FxHashMap::default(), choices: Vec::new(), spare_extras: Vec::new(), bounds, square: square_rect, gap, rows };
    // A mandatory catalogue request with no positive-weight eligible house can
    // never be removed from the queue. Built counts only grow and these weights
    // stay fixed; water is deliberately ignored, making this a necessary check.
    // Rejected layouts own their RNG, so no later layout loses any of its draws.
    if !required_houses_possible(&def.houses, &town.house_weights, &queue, &size.name, &style.name, rules.houses.grand) {
        return give_up(5);
    }
    // On a green, houses first take the rim, facing in.
    if shape == Shape::Green {
        let rim = (square as f32 + lanes_def.rim.0 + gap as f32).max(lanes_def.rim.1);
        let mut angle = start_angle;
        let mut tries = 0;
        let rim_houses = homes as usize / 2;
        while tries < lanes_def.rim.2 && queue.len() > rim_houses {
            tries += 1;
            angle += 0.45 + rng.unit() * 0.5;
            let p = centre.as_vec2() + Vec2::from_angle(angle) * rim;
            if let Some(lot) = place(&mut town, &mut ground, &mut rng, &queue[0], None, p, Facing::toward(centre.as_vec2() - p), &occupied, &lanes, None) {
                occupied.extend(lot.taken());
                town.count(&lot);
                lots.push(lot);
                queue.remove(0);
            }
        }
    }
    let lot_rules = &rules.lots;
    let mut spacing = lot_rules.spacing.0 + lot_rules.spacing.1 * (1.0 - crowding);
    // Lanes are walked pass by pass; where homes are still wanted after them, new side
    // lanes branch off and are walked in turn, a few times at most.
    let mut walked = 0;
    let mut grown = 0;
    let mut pass = 0;
    while !queue.is_empty() {
        if pass >= lot_rules.passes {
            let homes_left = queue.iter().any(|w| matches!(w, Want::Home | Want::Kind(_) | Want::Work(_)));
            if grown >= lot_rules.passes || !homes_left {
                break;
            }
            grown += 1;
            walked = lanes.len();
            // Side lanes off the lanes there are, round the buildings already standing.
            let street = lanes.clone();
            for k in 0..lanes_def.growth.0 {
                let lane = &street[rng.below(street.len() as u32) as usize];
                let (p, dir) = lane.along(lane.length() * (0.2 + 0.7 * rng.unit()));
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                let angle = dir.to_angle() + side * std::f32::consts::FRAC_PI_2 + (rng.unit() - 0.5) * 0.4 * (1.0 - order);
                let length = between(&mut rng, size.lane_length) as f32 * lanes_def.growth.1;
                let start = p + Vec2::from_angle(angle) * (lane.half + 0.5);
                let avoid: Vec<(Rect, f32)> = occupied.iter().map(|r| (*r, CLEAR)).chain(kept.iter().map(|r| (*r, side_half + RING as f32 + 0.5))).chain([(square_rect, CLEAR)]).collect();
                let points = walk(&mut ground, &lanes, &mut rng, start, angle, length, wiggle, &bounds, lanes_def, &avoid);
                if points.len() >= 4 {
                    let mut points = points;
                    points.insert(0, p);
                    lanes.push(Lane::new(points, side_half, false, false));
                }
            }
            if lanes.len() == walked {
                break;
            }
        }
        let from = if pass >= lot_rules.passes { walked } else { 0 };
        for li in from..lanes.len() {
            let lane = lanes[li].clone();
            let total = lane.length();
            let mut s = 3.0 + rng.unit() * 3.0 + pass.min(lot_rules.passes) as f32 * 2.0;
            let mut side = if rng.chance(0.5) { 1.0f32 } else { -1.0 };
            while s < total && !queue.is_empty() {
                let (p, dir) = lane.along(s);
                let normal = Vec2::new(-dir.y, dir.x) * side;
                let setback = lane.half + lot_rules.setback.0 + ((1.0 - crowding) * lot_rules.setback.1 * rng.unit()).round();
                let front = p + normal * setback;
                // The first of the next few wanted that fits here.
                let mut placed = false;
                let mut row = 0.0;
                for k in 0..queue.len().min(3) {
                    if let Some(lot) = place(&mut town, &mut ground, &mut rng, &queue[k], None, front, Facing::toward(-normal), &occupied, &lanes, Some(p)) {
                        let home = matches!(queue[k], Want::Home);
                        occupied.extend(lot.taken());
                        town.count(&lot);
                        lots.push(lot);
                        queue.remove(k);
                        placed = true;
                        if home {
                            let first = lots.len();
                            row = grow_row(&mut town, &mut ground, &mut lots, &mut occupied, &mut queue, &lanes, dir);
                            *rowed |= lots.len() > first;
                        }
                        break;
                    }
                }
                s += if placed { spacing * (0.6 + 0.8 * rng.unit()) + row } else { 2.0 };
                // Orderly places alternate sides; others wander.
                if order > 0.5 || rng.chance(0.5) {
                    side = -side;
                }
            }
        }
        if pass < lot_rules.passes {
            spacing *= lot_rules.closer;
        }
        pass += 1;
    }
    #[cfg(test)]
    phase_end("main lots (including rejected attempts)", lots_time);
    #[cfg(test)]
    let outskirts_time = phase_start();
    // A place missing what it must have (its inn, smithy or trader, the works it always
    // has, or too few homes) is not built.
    let left_homes = queue.iter().filter(|w| matches!(w, Want::Home)).count() as i32;
    if queue.iter().any(|w| matches!(w, Want::Kind(_) | Want::Work(_))) {
        return give_up(5);
    }
    if homes - left_homes < size.houses.0 as i32 {
        return give_up(6);
    }
    // Out at the ends of the lanes, and a centrepiece that stands there.
    let edge = centre_spec.filter(|_| !on_square);
    outskirts.sort_by_key(|k| !matches!(k, Kind::Field | Kind::Pen));
    let outskirts: Vec<(Kind, Option<Spec>)> = edge.into_iter().map(|s| (Kind::Centrepiece, Some(s))).chain(outskirts.into_iter().map(|k| (k, None))).collect();
    for (kind, given) in outskirts {
        // The field or pen a place must have is looked for harder, anywhere along its lanes.
        let farmed = lots.iter().any(|l| matches!(l.spec.kind, Kind::Field | Kind::Pen));
        let must = !farmed && matches!(kind, Kind::Field | Kind::Pen);
        let (tries, from, back) = if must { (lot_rules.tries * lot_rules.farm.0, lot_rules.farm.1, lot_rules.farm.2) } else { (lot_rules.tries, lot_rules.outskirts.0, lot_rules.outskirts.1) };
        for _ in 0..tries {
            let lane = &lanes[rng.below(lanes.len() as u32) as usize];
            let total = lane.length();
            let s = total * (from + (1.0 - from) * rng.unit());
            let (p, dir) = lane.along(s);
            let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
            let normal = Vec2::new(-dir.y, dir.x) * side;
            let front = p + normal * (lane.half + 2.0 + rng.unit() * back);
            if let Some(mut lot) = place(&mut town, &mut ground, &mut rng, &Want::Kind(kind), given.clone(), front, Facing::toward(-normal), &occupied, &lanes, None) {
                if kind == Kind::Centrepiece {
                    lot.centre = centrepiece;
                    kept.push(lot.rect);
                }
                occupied.extend(lot.taken());
                lots.push(lot);
                break;
            }
        }
    }
    // Every place keeps animals or grows food.
    if !lots.iter().any(|l| matches!(l.spec.kind, Kind::Field | Kind::Pen)) {
        return give_up(7);
    }
    // The whole place, not only its middle, stands in land it may: no avoided biome or
    // climate under any building, and fertile enough ground under them all.
    let mut fertility = 0.0;
    for lot in &lots {
        let middle = (lot.rect.min + lot.rect.max) / 2;
        let ground_site = ctx.world.site(middle.x, middle.y);
        if !habitable(ctx, &ground_site) {
            return give_up(8);
        }
        fertility += ground_site.fertility;
    }
    let least = def.site.poor.iter().filter(|(tag, _)| biome.has_tag(tag)).map(|(_, &f)| f).fold(def.site.fertility, f32::min);
    if fertility / (lots.len() as f32) < least {
        return give_up(9);
    }
    #[cfg(test)]
    phase_end("outskirts/habitability", outskirts_time);
    #[cfg(test)]
    let props_time = phase_start();

    // A wall round the heart of a walled place (its size's `walls`, `rules.walls`): its
    // square and buildings, but not the fields, pens, orchards, woodyards and shrines out
    // at the ends, nor a centrepiece out there. A gate where a lane passes; none where it
    // would cut a building or stand in water; towers at its corners and along it.
    let mut walls_rng = Rng::new(ctx.seed, WALLS, cell.x, cell.y);
    let walled = size.walls.0 > 0.0 && walls_rng.chance(size.walls.1.weigh(size.walls.0, &levers).min(1.0));
    let wall_rules = &rules.walls;
    let mut wall: Vec<Walling> = Vec::new();
    if walled {
        // Round the heart, and round any field, pen or shrine out at the ends the ring would
        // cut through, so it is never left open beside one.
        let mut inner: Vec<bool> = lots.iter().map(|l| !matches!(l.spec.kind, Kind::Field | Kind::Pen | Kind::Orchard | Kind::Woodyard | Kind::Shrine | Kind::Centrepiece)).collect();
        let (corners, cells) = loop {
            let heart: Vec<Rect> = lots.iter().zip(&inner).filter(|(_, inner)| **inner).map(|(l, _)| l.rect).chain([square_rect]).collect();
            let corners = super::walls::ring(&heart, wall_rules.margin);
            let cells = super::walls::trace(&corners);
            let mut grew = false;
            for (l, inner) in lots.iter().zip(inner.iter_mut()) {
                if !*inner && cells.iter().any(|&p| l.rect.grow(1).holds(p)) {
                    *inner = true;
                    grew = true;
                }
            }
            if !grew {
                break (corners, cells);
            }
        };
        let on_lot = |p: IVec2| lots.iter().any(|l| l.rect.grow(1).holds(p));
        #[cfg(test)]
        WALL_ON_LOTS.fetch_add(cells.iter().filter(|&&p| on_lot(p)).count() as u32, std::sync::atomic::Ordering::Relaxed);
        let blocked = |ground: &mut Ground, p: IVec2| !bounds.holds(p) || on_lot(p) || !standing(&ground.at(p));
        // A lane leading out that ends within the ring (or just past it) runs on out, to
        // `GATE_ROAD` blocks past the wall, so the trail joining its end comes in at a gate
        // and not over the wall: never over a lot, nor out of the place's bounds.
        let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
        let inside = |p: Vec2| corners.len() >= 3 && {
            let sides: Vec<f32> = (0..corners.len()).map(|i| cross(corners[i].as_vec2(), corners[(i + 1) % corners.len()].as_vec2(), p)).collect();
            sides.iter().all(|&c| c >= 0.0) || sides.iter().all(|&c| c <= 0.0)
        };
        // Straight on if it can, else turning a little at a time; a lane that cannot get
        // out no longer leads out (no trail joins it), so long as another does.
        let mut shut = Vec::new();
        for (i, lane) in lanes.iter_mut().enumerate().filter(|(_, l)| l.main) {
            let n = lane.points.len();
            let near = |p: Vec2| inside(p) || cells.iter().any(|c| c.as_vec2().distance(p) < GATE_ROAD);
            if n < 2 || !near(lane.points[n - 1]) {
                continue;
            }
            let (end, ahead) = (lane.points[n - 1], (lane.points[n - 1] - lane.points[n - 2]).normalize_or_zero());
            let half = lane.half;
            let out = [0.0f32, 30.0, -30.0, 60.0, -60.0, 90.0, -90.0].into_iter().filter(|_| ahead != Vec2::ZERO).find_map(|turn| {
                let dir = Vec2::from_angle(turn.to_radians()).rotate(ahead);
                let out = (1..=96).map(|t| end + dir * t as f32).find(|&p| !inside(p) && cells.iter().all(|c| c.as_vec2().distance(p) >= GATE_ROAD))?;
                let steps = end.distance(out).ceil() as i32;
                (0..=steps)
                    .map(|k| end.lerp(out, k as f32 / steps.max(1) as f32))
                    .all(|q| bounds.holds(q.round().as_ivec2()) && lots.iter().all(|l| l.rect.distance(q) > half + 0.5))
                    .then_some(out)
            });
            match out {
                Some(out) => {
                    let mut points = lane.points.clone();
                    points.push(out);
                    *lane = Lane::new(points, lane.half, lane.paved, lane.main);
                }
                None => shut.push(i),
            }
        }
        if lanes.iter().filter(|l| l.main).count() > shut.len() {
            for i in shut {
                lanes[i].main = false;
            }
        }
        let crossed = |p: IVec2, by: f32| lanes.iter().any(|l| l.distance(p.as_vec2()) <= l.half + by);
        let gates: Vec<bool> = cells.iter().map(|&p| crossed(p, 1.0)).collect();
        for (i, &p) in cells.iter().enumerate() {
            if gates[i] || blocked(&mut ground, p) {
                continue;
            }
            let n = cells.len();
            let post = gates[(i + 1) % n] || gates[(i + n - 1) % n];
            wall.push(Walling { at: p, part: if post { WallPart::Post } else if i % 2 == 0 { WallPart::Merlon } else { WallPart::Wall } });
        }
        // Lintels over the gates where the wall is high enough to walk under one.
        for (i, &p) in cells.iter().enumerate() {
            if gates[i] && !blocked(&mut ground, p) {
                wall.push(Walling { at: p, part: WallPart::Lintel });
            }
        }
        for i in super::walls::towers(&cells, &corners, wall_rules.towers.0 as usize) {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = cells[i] + IVec2::new(dx, dz);
                    // Never on a gate (its lintel), so as wide a berth of a lane as a gate's.
                    if crossed(p, 1.0) || blocked(&mut ground, p) {
                        continue;
                    }
                    let part = if dx != 0 && dz != 0 { WallPart::TowerCorner } else { WallPart::Tower };
                    wall.retain(|w| w.at != p);
                    wall.push(Walling { at: p, part });
                }
            }
        }
    }
    let walled_at: rustc_hash::FxHashSet<IVec2> = wall.iter().filter(|w| w.part != WallPart::Lintel).map(|w| w.at).collect();

    // Props by the lanes and in the yards, and by the water.
    let mut canvas = Canvas::default();
    let mut props: Vec<(Rect, usize, Facing)> = Vec::new();
    let prop_weights: Vec<(usize, f32)> = def.props.iter().enumerate().filter(|(_, p)| !p.water).map(|(i, p)| (i, p.lean.weigh(p.weight, &levers) * culture.mult(Listing::Prop, &p.kind.label()))).collect();
    let water_weights: Vec<(usize, f32)> = def.props.iter().enumerate().filter(|(_, p)| p.water).map(|(i, p)| (i, p.lean.weigh(p.weight, &levers) * culture.mult(Listing::Prop, &p.kind.label()))).collect();
    let lamp = def.props.iter().position(|p| p.kind == PropKind::Drawn(Prop::Lamp));
    // Where a prop would stand with its first block at `at`, if the ground there is free.
    let fit = |ground: &mut Ground, occupied: &[Rect], prop: usize, at: IVec2, facing: Facing, clearance: f32| -> Option<Rect> {
        let (low, high) = prop_box(&def.props[prop].kind);
        let rect = match &def.props[prop].kind {
            PropKind::Drawn(_) => Rect::new(at, Frame::extent(low, high, facing)),
            PropKind::Blocks(..) => Rect { min: at + low, max: at + high },
        };
        let free = bounds.covers(&rect)
            && occupied.iter().all(|o| o.gap(&rect) >= 0)
            && lanes.iter().all(|l| !l.rect_any(&rect, |d| d <= l.half + clearance))
            && (walled_at.is_empty() || !(rect.min.y - 1..=rect.max.y + 1).any(|z| (rect.min.x - 1..=rect.max.x + 1).any(|x| walled_at.contains(&IVec2::new(x, z)))))
            && [rect.min, rect.max, IVec2::new(rect.min.x, rect.max.y), IVec2::new(rect.max.x, rect.min.y)].iter().all(|&c| walkable(&ground.at(c)));
        free.then_some(rect)
    };
    let dressing = &rules.dressing;
    let lamps_every = (dressing.every.0 + dressing.every.1 * (1.0 - wealth)) * size.lamps;
    for lane in &lanes {
        let total = lane.length();
        let mut s = 4.0 + rng.unit() * lamps_every;
        while s < total {
            let (p, dir) = lane.along(s);
            let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
            let at = (p + Vec2::new(-dir.y, dir.x) * side * (lane.half + 1.0)).round().as_ivec2();
            let facing = Facing::toward(p - at.as_vec2());
            let prop = if rng.chance(dressing.lamp.0 + dressing.lamp.1 * wealth) && lamp.is_some() { lamp } else { pick(&mut rng, &prop_weights) };
            if let Some(prop) = prop
                && let Some(rect) = fit(&mut ground, &occupied, prop, at, facing, dressing.clearance.0)
            {
                props.push((rect, prop, facing));
                occupied.push(rect);
            }
            s += lamps_every * (0.7 + 0.6 * rng.unit());
        }
    }
    // A few more in yards.
    let yard_props = (lots.len() as f32 * (dressing.yards.0 + dressing.yards.1 * levers.get(Lever::Green)) * rng.unit()).round() as usize + dressing.yards.2 as usize;
    for _ in 0..yard_props * 3 {
        if props.len() > yard_props + lanes.len() * dressing.per_lane as usize {
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
            && let Some(rect) = fit(&mut ground, &occupied, prop, at, facing, dressing.clearance.1)
        {
            props.push((rect, prop, facing));
            occupied.push(rect);
        }
    }
    // Boats and nets on the bank of the water near.
    if fresh && !water_weights.is_empty() {
        let reach = lots.iter().map(|l| l.rect.distance(centre.as_vec2()) as i32).max().unwrap_or(0) + 8;
        let mut banks = between(&mut rng, dressing.banks);
        for spot in banks_near(&mut ground, centre, reach) {
            if banks == 0 {
                break;
            }
            let Some(prop) = pick(&mut rng, &water_weights) else { break };
            let facing = Facing::toward(spot.1);
            if let Some(rect) = fit(&mut ground, &occupied, prop, spot.0, facing, dressing.clearance.1) {
                props.push((rect, prop, facing));
                occupied.push(rect);
                banks -= 1;
            }
        }
    }

    // Draw everything.
    #[cfg(test)]
    {
        phase_end("props", props_time);
        phase_end("lanes/lots/props", time);
        if PROFILE.with(|p| p.get()) {
            eprintln!("  plan ground: {} cached samples, {} queries including retries", ground.seen.len(), ground.queries);
        }
    }
    #[cfg(test)]
    let time = phase_start();
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
    // The wall: stone where the place is rich, else a palisade of its logs; merlons on
    // every other block, posts either side of each gate, a lintel over it.
    let stone = wealth > wall_rules.stone;
    let wall_height = if stone { wall_rules.height.1 } else { wall_rules.height.0 } as i32;
    let wall_block = (!wall.is_empty()).then(|| {
        let mats = kit.mats(ctx, &mut walls_rng);
        if stone { mats.stone() } else { mats.log }
    });
    let mut walls_out: Vec<(IVec2, i32)> = Vec::with_capacity(wall.len());
    if let Some(block) = wall_block {
        let tower = wall_height + wall_rules.towers.1 as i32;
        for w in &wall {
            let (from, to) = match w.part {
                WallPart::Wall => (1, wall_height),
                WallPart::Merlon => (1, wall_height + i32::from(stone)),
                WallPart::Post => (1, wall_height + 1),
                // Three blocks clear under it, and only where there is wall above that.
                WallPart::Lintel if wall_height > 3 => (4, wall_height),
                WallPart::Lintel => continue,
                WallPart::Tower => (1, tower),
                WallPart::TowerCorner => (1, tower + 1),
            };
            for up in from..=to {
                canvas.on_ground(w.at.x, w.at.y, up, Put::Set(block));
            }
            if w.part != WallPart::Lintel {
                walls_out.push((w.at, to));
            }
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
    let pave_square = shape != Shape::Green && wealth > size.paving.0 && !kit.paving.is_empty();
    let seed = hash3(ctx.seed, cell.x, 7, cell.y);
    if pave_square {
        let r = square as f32 + 0.5;
        for z in -square - 1..=square + 1 {
            for x in -square - 1..=square + 1 {
                let p = centre + IVec2::new(x, z);
                let d = p.as_vec2().distance(centre.as_vec2());
                let ragged = unit(hash3(seed ^ 0x5a, p.x, 0, p.y)) * 1.6 * (1.0 - order + 0.3);
                // Not under anything built: paving lands after the buildings, so it would
                // cap a well's water, a pond or a fountain.
                let under = lots.iter().any(|l| l.rect.distance(p.as_vec2()) < 0.5);
                if d <= r - ragged
                    && !under
                    && let Some(block) = kit.paving.at(seed, IVec3::new(p.x, 0, p.y))
                {
                    canvas.on_ground(p.x, p.y, 0, Put::Set(block));
                }
            }
        }
    }

    #[cfg(test)]
    phase_end("buildings/props/square paving", time);
    #[cfg(test)]
    let detail_time = phase_start();
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
    for &(p, _) in &walls_out {
        min = min.min(p - 2);
        max = max.max(p + 2);
    }
    let mut raster = Raster::new(min, max);
    // The square: flat in the middle, easing out.
    raster.pad(&Rect { min: centre - square, max: centre + square }, square_height, FALLOFF + 1, 0, true, |p| p.as_vec2().distance(centre.as_vec2()) <= square as f32 + 0.5);
    for lot in &lots {
        let grade = !matches!(lot.spec.kind, Kind::Stall | Kind::Shrine | Kind::Pen | Kind::Orchard | Kind::Woodyard);
        let core = lot.rect.grow(if grade { 1 } else { 0 });
        if grade {
            raster.pad(&core, lot.floor, FALLOFF, lot.yard, true, |_| true);
        } else {
            raster.clear_rect(&lot.rect.grow(lot.yard));
        }
        buildings.push(Building {
            kind: lot.spec.kind,
            min: lot.rect.min,
            max: lot.rect.max,
            floor: lot.floor + lot.spec.stilts,
            room: lot.spec.room(&lot.frame),
            byre: lot.spec.byre_room(&lot.frame),
            // Two blocks out from the door: on its porch, or on the step below the top
            // one when it stands on stilts.
            door: lot.door.map(|(d, _)| {
                let up = if lot.spec.has(Extra::Porch) { lot.spec.stilts + 1 } else { (lot.spec.stilts - 1).max(0) + 1 };
                IVec3::new(d.x, lot.floor + up, d.y)
            }),
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
            beds: Vec::new(),
            seats: Vec::new(),
        });
    }
    // Ground held level round props that hold water.
    for &(rect, prop, _) in &props {
        if def.props[prop].kind == PropKind::Drawn(Prop::Washtub) {
            let height = ground.at(rect.min).height;
            // Its own little pad, never under a building's edge.
            raster.pad(&rect.grow(1), height, 2, 0, false, |_| true);
        }
    }
    raster.finish();
    for lane in &lanes {
        raster.lane(lane);
    }
    // A path from each door to its lane.
    for lot in &lots {
        if let Some((door, to)) = lot.door {
            let from = door.as_vec2();
            let path = Lane::bare(vec![from, to], 0.5, false, false);
            raster.lane(&path);
        }
    }
    for (rect, ..) in &props {
        raster.clear_rect(rect);
    }
    // No tree stands in the wall or against it.
    for &(p, _) in &walls_out {
        raster.clear_rect(&Rect { min: p - 1, max: p + 1 });
    }
    // A clearing in the woods, leafier where the place is green.
    let rects: Vec<Rect> = lots.iter().map(|l| l.rect).collect();
    let (near, far) = def.site.glade;
    let (trees, greener) = def.site.glade_trees;
    raster.glade(&rects, near, far, trees + greener * levers.get(Lever::Green), seed);
    #[cfg(test)]
    phase_end("raster/grading/lanes/glade", detail_time);
    #[cfg(test)]
    let detail_time = phase_start();
    // Paved lanes.
    for lane in lanes.iter().filter(|l| l.paved) {
        let Some(bounds) = lane.paving_bounds(Rect { min: raster.min, max: raster.max }) else { continue };
        for z in bounds.min.y..=bounds.max.y {
            for x in bounds.min.x..=bounds.max.x {
                let p = IVec2::new(x, z);
                if lane.distance(p.as_vec2()) <= lane.half
                    && !rects.iter().any(|r| r.distance(p.as_vec2()) < 0.5)
                    && let Some(block) = kit.paving.at(seed, IVec3::new(x, 0, z))
                {
                    canvas.grounded.entry(IVec3::new(x, 0, z)).or_insert(Put::Set(block));
                }
            }
        }
    }

    #[cfg(test)]
    phase_end("lane paving", detail_time);
    #[cfg(test)]
    let detail_time = phase_start();
    let gates: Vec<IVec2> = lanes.iter().filter(|l| l.main).map(|l| l.points.last().unwrap().round().as_ivec2()).collect();
    // Its people, and their homes furnished for them (`interiors`): drawn last, over the
    // houses as they stand.
    let size_name = &def.sizes[size_index].name;
    let people = super::people::residents(&def.people, &def.houses, size_name, &levers, &buildings, &gates, hash3(ctx.seed, cell.x, 7, cell.y));
    if let Some(interiors) = &ctx.fixed.interiors {
        for (i, lot) in lots.iter().enumerate() {
            if lot.spec.kind != Kind::House || !lot.spec.inside.is_empty() {
                continue;
            }
            let work = |w: Option<usize>| w.map(|w| buildings[w].work.clone().unwrap_or_else(|| buildings[w].kind.name().to_owned()));
            let home = super::interior::Household { people: people.iter().filter(|r| r.home == i).map(|r| (r.species.clone(), work(r.work))).collect() };
            let hearth = lot.spec.chimney().is_some();
            let furnished = super::interior::furnish(interiors, &lot.spec, &lot.frame, &kit.wood, &levers, &home, hearth, &mut canvas);
            buildings[i].beds = furnished.beds;
            buildings[i].seats = furnished.seats;
        }
    }
    // No flame of its own stands within reach of what catches (thatch): such a roof is
    // laid in shingles instead, and a flame that still reaches is put out.
    let (_relaid, _put_out) = ctx.fixed.fire.keep_from_flames(&mut canvas, |x, z| raster.ground(x, z, &ground.at(IVec2::new(x, z))));
    #[cfg(test)]
    {
        ROOFS_RELAID.fetch_add(_relaid as u32, std::sync::atomic::Ordering::Relaxed);
        FLAMES_PUT_OUT.fetch_add(_put_out as u32, std::sync::atomic::Ordering::Relaxed);
    }
    let voxels = canvas.into_columns();
    #[cfg(test)]
    phase_end("voxel columns", detail_time);
    #[cfg(test)]
    phase_end("drawing/raster/voxel columns", time);
    #[cfg(test)]
    let name_time = phase_start();
    let name = unique_name(def, ctx.seed, ctx.promised, cell, list, small_name);
    let name = match prefix {
        (true, Some(i)) => {
            let p = &names.prefixes[i];
            let joiner = if names.script == Script::Kanji { "" } else { "・" };
            (format!("{} {}", p.0, name.0), format!("{}{joiner}{}", p.1, name.1))
        }
        _ => name,
    };
    #[cfg(test)]
    phase_end("name", name_time);
    Some(Plan {
        id: hash3(ctx.seed, cell.x, 0, cell.y),
        size: def.sizes[size_index].name.clone(),
        layout: def.layouts[layout_index].name.clone(),
        style: style.name.clone(),
        name,
        centre,
        height: square_height,
        levers,
        culture,
        buildings,
        gates,
        lanes: lanes.iter().map(|l| l.points.clone()).collect(),
        people,
        lane_halves: lanes.iter().map(|l| l.half).collect(),
        walls: walls_out,
        wall_block,
        min: raster.min,
        max: raster.max,
        raster,
        voxels,
    })
}

/// A name's two parts joined in Japanese: where the English runs a consonant into a vowel
/// (Sab + ir: Sabir) the kana merge the same way (サブ + イル: サビル, not サブイル), by
/// the `kana` table in settlements.ron. A consonant kana shared by the two parts is
/// written once (Oak + combe: オークーム), preserving vowel and long-mark sequences.
pub(super) fn join_kana(kana: &[KanaJoin], first: &(String, String), last: &(String, String)) -> String {
    if let Some(shared) = first.1.chars().last()
        && shared != 'ー' && !"アイウエオ".contains(shared)
        && last.1.starts_with(shared)
    {
        return format!("{}{}", first.1, &last.1[shared.len_utf8()..]);
    }
    let vowel = |c: Option<char>| c.is_some_and(|c| "aeiou".contains(c.to_ascii_lowercase()));
    let runs_on = !vowel(first.0.chars().last()) && first.0.chars().last().is_some_and(|c| c.is_ascii_alphabetic() && c != 'y') && vowel(last.0.chars().next());
    let opening = last.1.chars().next().and_then(|c| "アイウエオ".chars().position(|v| v == c));
    if let (true, Some(opening)) = (runs_on, opening) {
        let english = first.0.to_lowercase();
        if let Some(join) = kana.iter().find(|j| first.1.ends_with(&j.kana) && english.ends_with(&j.after)) {
            let stem = &first.1[..first.1.len() - join.kana.len()];
            let rest: String = last.1.chars().skip(1).collect();
            return format!("{stem}{}{rest}", join.into[opening]);
        }
    }
    format!("{}{}", first.1, last.1)
}

/// How far graded ground eases back into the land around, in blocks.
pub const FALLOFF: i32 = 5;

/// How far walked lanes keep from what they avoid, centre line to blocks.
const CLEAR: f32 = 2.0;

/// The ring of blocks kept clear of lanes round a well or centrepiece.
const RING: i32 = 1;


/// Walks a lane from `from`, heading `angle`, for `length` blocks: it bends a little at
/// random, more in disorderly places, and steers round water, steep ground and other
/// lanes, ending early where it cannot go on.
#[allow(clippy::too_many_arguments)]
fn walk(ground: &mut Ground, lanes: &[Lane], rng: &mut Rng, from: Vec2, angle: f32, length: f32, wiggle: f32, bounds: &Rect, rules: &LanesDef, avoid: &[(Rect, f32)]) -> Vec<Vec2> {
    let step = rules.step;
    let mut points = vec![from];
    let mut heading = angle;
    let mut bend = 0.0f32;
    let mut here = from;
    let mut height = ground.at(from.round().as_ivec2()).height;
    let mut walked = 0.0;
    while walked < length {
        bend = (bend + (rng.unit() - 0.5) * wiggle * 0.5).clamp(-wiggle, wiggle);
        heading += bend;
        let mut choices = [None; 5];
        let mut samples = [IVec2::ZERO; 5];
        let mut count = 0;
        for (i, turn) in [-0.45f32, -0.2, 0.0, 0.2, 0.45].into_iter().enumerate() {
            let a = heading + turn;
            let next = here + Vec2::from_angle(a) * step;
            let at = next.round().as_ivec2();
            // Never nearer than `clear` to what it avoids, nor nearer than it is already
            // when it sets out closer, as lanes off the square do.
            if !bounds.holds(at) { continue; }
            let clearance = StepClearance::new(here, next);
            if avoid.iter().any(|(r, clear)| clearance.nears(r, *clear)) {
                continue;
            }
            choices[i] = Some((turn, a, next, at));
            samples[count] = at;
            count += 1;
        }
        // Every geometrically eligible turn is sampled before choosing the best;
        // no RNG or terrain-dependent decision changes the later turns' inclusion.
        ground.prefetch_points(&samples[..count]);
        let mut best: Option<(f32, f32, Vec2, i32)> = None;
        for (turn, a, next, at) in choices.into_iter().flatten() {
            let g = ground.at(at);
            if !walkable(&g) {
                continue;
            }
            let rise = (g.height - height).abs();
            if rise > rules.rise {
                continue;
            }
            // Not into another lane, past its first few blocks.
            if walked > rules.leave && lanes.iter().any(|l| l.distance(next) < l.half + rules.apart) {
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
        walked += step;
    }
    points
}

/// The original seven rounded step samples, shared across every avoided rectangle.
/// Their actual extrema give a conservative signed-distance lower bound: each axis
/// is no greater than that of any sampled point, including inside a rectangle. A
/// passing bound still uses all original point distances and the MAX fold sentinel.
struct StepClearance {
    from: Vec2,
    points: [Vec2; 7],
    bounds: Option<(Vec2, Vec2)>,
}

impl StepClearance {
    fn new(from: Vec2, to: Vec2) -> Self {
        let points = std::array::from_fn(|k| from.lerp(to, k as f32 / 6.0));
        let bounds = points.iter().all(|p| p.is_finite()).then(|| {
            points.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(min, max), &p| (min.min(p), max.max(p)))
        });
        Self { from, points, bounds }
    }

    fn nears(&self, rect: &Rect, clear: f32) -> bool {
        let threshold = clear.min(rect.signed(self.from) - 0.01);
        if let Some((min, max)) = self.bounds {
            let d = (rect.min.as_vec2() - 0.5 - max).max(min - (rect.max.as_vec2() + 0.5));
            let lower = if d.max_element() > 0.0 { d.max(Vec2::ZERO).length() } else { d.max_element() };
            if lower.min(f32::MAX) >= threshold { return false; }
        }
        self.points.iter().map(|&p| rect.signed(p)).fold(f32::MAX, f32::min) < threshold
    }
}

/// Rolls a special building's spec: `w` by `d`, or (when nothing) its size in `rules`.
#[allow(clippy::too_many_arguments)]
fn special_spec(kind: Kind, w: i32, d: i32, kit: &Kit, levers: &Levers, rng: &mut Rng, style: &StyleDef, rules: &RulesDef) -> Spec {
    let (w, d) = match rules.specials.get(kind.name()).filter(|_| w == 0) {
        Some(size) => {
            let roll = |rng: &mut Rng, (lo, hi): (u32, u32)| lo as i32 + rng.below((hi - lo) / size.step + 1) * size.step as i32;
            let w = roll(rng, size.width);
            (w, roll(rng, size.depth))
        }
        None => (w.max(3), d.max(3)),
    };
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
        regular: levers.get(Lever::Order) > rules.houses.regular,
        window_chance: style.window_chance,
        form: Form::Box,
        inside: Vec::new(),
        centre: None,
        seed: rng.next_u64(), row: None, byre: 0,
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
    let mut spec = special_spec(Kind::Centrepiece, w, d, kit, levers, rng, style, &def.rules);
    spec.centre = drawn;
    spec
}

/// The direction of the nearest fresh water within `reach` blocks, if any.
fn water_near(ground: &mut Ground, centre: IVec2, reach: i32) -> Option<Vec2> {
    for r in (8..=reach).step_by(8) {
        let ring: [(Vec2, IVec2); 24] = std::array::from_fn(|k| {
            let dir = Vec2::from_angle(k as f32 * std::f32::consts::TAU / 24.0);
            (dir, centre + (dir * r as f32).round().as_ivec2())
        });
        // The original decision sums all 24 points before testing this ring.
        ground.prefetch_points(&ring.map(|(_, p)| p));
        let mut sum = Vec2::ZERO;
        for (dir, p) in ring {
            let g = ground.at(p);
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
    ground.prefetch_points(&[centre + IVec2::new(16, 0), centre + IVec2::new(-16, 0), centre + IVec2::new(0, 16), centre + IVec2::new(0, -16)]);
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
        for first in (0..n).step_by(crate::water::SAMPLE_BATCH) {
            let count = (n - first).min(crate::water::SAMPLE_BATCH as i32) as usize;
            let mut ring = [IVec2::ZERO; crate::water::SAMPLE_BATCH];
            for (i, at) in ring[..count].iter_mut().enumerate() {
                let k = first + i as i32;
                *at = centre + (Vec2::from_angle(k as f32 * std::f32::consts::TAU / n as f32) * r as f32).round().as_ivec2();
            }
            let points = &ring[..count];
            // This ring is always fully inspected before the >=8 stop below.
            ground.prefetch_points(points);
            for &at in points {
                if !standing(&ground.at(at)) {
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
    house_weights: Vec<f32>,
    extra_chances: Vec<f32>,
    style: &'a StyleDef,
    size: &'a str,
    built: FxHashMap<usize, u32>,
    /// Availability changes only after a lot is counted; rejected attempts reuse
    /// the original ordered choices, including zero-weight entries.
    choices: Vec<HouseChoices>,
    /// One rejected house's allocation, cleared before the next original roll.
    spare_extras: Vec<Extra>,
    /// What it may build on: inside `bounds`, clear of the square, `gap` between.
    bounds: Rect,
    square: Rect,
    gap: i32,
    /// Whether homes may be built on to each other in rows (`StyleDef::row`).
    rows: bool,
}

/// What is wanted of a lot.
#[derive(Clone, Debug, PartialEq)]
enum Want {
    /// A kind of building: the inn, smithy, trader, or one out at the ends.
    Kind(Kind),
    /// A home, of the houses with no work.
    Home,
    /// A working building, of the houses with work.
    Workshop,
    /// A working building doing this work.
    Work(String),
}

struct HouseChoices {
    want: Want,
    dry: Vec<(usize, f32)>,
    wet: Vec<(usize, f32)>,
    needs_water: bool,
}

impl HouseChoices {
    fn new(houses: &[super::def::HouseDef], weights: &[f32], want: &Want, size: &str, style: &str, built: &FxHashMap<usize, u32>, grand: u32) -> Self {
        let mut choices = Self { want: want.clone(), dry: Vec::new(), wet: Vec::new(), needs_water: false };
        for (i, h) in houses.iter().enumerate() {
            if !house_fits(h, want, size, style, built.get(&i).copied().unwrap_or(0), grand) { continue; }
            choices.wet.push((i, weights[i]));
            if h.water { choices.needs_water = true; } else { choices.dry.push((i, weights[i])); }
        }
        choices
    }
}

impl Town<'_> {
    fn count(&mut self, lot: &Lot) {
        if let Some(i) = lot.house {
            *self.built.entry(i).or_default() += 1;
            self.choices.clear();
        }
    }

    fn choices(&mut self, want: &Want) -> &HouseChoices {
        let i = self.choices.iter().position(|c| &c.want == want).unwrap_or_else(|| {
            self.choices.push(HouseChoices::new(&self.ctx.def.houses, &self.house_weights, want, self.size, &self.style.name, &self.built, self.ctx.def.rules.houses.grand));
            self.choices.len() - 1
        });
        &self.choices[i]
    }

    /// Water cannot affect a request whose otherwise eligible houses are all dry.
    fn needs_water(&mut self, want: &Want) -> bool {
        self.choices(want).needs_water
    }

    fn reject(&mut self, spec: Spec, reason: usize) -> Option<Lot> {
        if spec.extras.capacity() > self.spare_extras.capacity() { self.spare_extras = spec.extras; }
        miss(reason)
    }
}

/// Availability before testing the water: identical for selection and deciding whether
/// a water search can affect it. Built counts only grow while a layout is being placed.
fn house_fits(h: &super::def::HouseDef, want: &Want, size: &str, style: &str, built: u32, grand: u32) -> bool {
    (h.sizes.is_empty() || h.sizes.iter().any(|s| s == size))
        && !h.not_in.iter().any(|s| s == style)
        && h.most.is_none_or(|most| built < most)
        && match want {
            Want::Kind(Kind::Inn) => h.work.is_none() && h.form == Form::Box && h.width.1 >= grand,
            Want::Kind(Kind::Trader) => h.work.is_none() && matches!(h.form, Form::Box | Form::ShopFront),
            Want::Home | Want::Kind(_) => h.work.is_none(),
            Want::Workshop => h.work.is_some(),
            Want::Work(work) => h.work.as_deref() == Some(work.as_str()),
        }
}

fn required_houses_possible(houses: &[super::def::HouseDef], weights: &[f32], queue: &[Want], size: &str, style: &str, grand: u32) -> bool {
    queue.iter().filter(|want| matches!(want, Want::Work(_) | Want::Kind(Kind::Inn | Kind::Trader))).all(|want| {
        houses.iter().zip(weights).any(|(house, &weight)| weight > 0.0 && house_fits(house, want, size, style, 0, grand))
    })
}

/// Rolls a house of the settlement's kinds for `want`, with its extras: one the place's
/// size builds, of which it has not built its most, beside water if it must be; none when
/// nothing fits. Inns are grand homes of four walls, by weight among those wide enough;
/// traders keep a modest home or a shop.
fn house_spec(town: &mut Town, want: &Want, kind: Kind, rng: &mut Rng, near_water: bool) -> Option<(Spec, usize)> {
    let choices = town.choices(want);
    let index = pick(rng, if near_water { &choices.wet } else { &choices.dry })?;
    let (def, kit, levers, style) = (town.ctx.def, town.kit, town.levers, town.style);
    let rules = &def.rules.houses;
    let h = &def.houses[index];
    let mut w = between(rng, h.width);
    let mut d = between(rng, h.depth);
    let mut storeys = between(rng, h.storeys);
    let mut wall = between(rng, h.walls);
    if kind == Kind::Inn {
        let (least_storeys, least_w, least_d) = rules.inn;
        storeys = storeys.max(least_storeys as i32);
        w = w.max(least_w as i32);
        d = d.max(least_d as i32);
    }
    if kind == Kind::Trader {
        let ((w0, w1), (d0, d1)) = rules.trader;
        w = w.clamp(w0 as i32, w1 as i32);
        d = d.clamp(d0 as i32, d1 as i32);
    }
    let mut extras = std::mem::take(&mut town.spare_extras);
    extras.clear();
    for (e, &chance) in def.extras.iter().zip(&town.extra_chances) {
        if rng.chance(chance) {
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
    let pitch = if !steep && w.max(d) >= rules.pitch.0 as i32 && rng.chance(rules.pitch.1) { 2 } else { 1 };
    let boxy = matches!(h.form, Form::Box | Form::ShopFront);
    let wing = (boxy && (rng.chance(h.wing) || extras.contains(&Extra::Wing))).then(|| (3 + rng.below(2), 3 + rng.below(3))).filter(|(ww, _)| *ww < w - 1);
    let shed = if boxy && extras.contains(&Extra::Shed) && d >= 5 { 3 + rng.below((d - 4).min(3) as u32) } else { 0 };
    let door = if levers.get(Lever::Order) > rules.middle_door || !boxy { w / 2 } else { rng.range(1, w - 1) };
    // A byre at the back of a home of one storey on the ground, keeping its people's rows.
    let stilts = h.stilts.unwrap_or(style.stilts) as i32;
    let byre = match h.byre {
        Some(b) if kind == Kind::House && storeys == 1 && wing.is_none() && stilts == 0 && rng.chance(b.chance) => {
            let rows = between(rng, b.rows).min(d - BYRE_KEEPS as i32 - 3);
            if rows >= b.rows.0 as i32 { rows } else { 0 }
        }
        _ => 0,
    };
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
        stilts,
        regular: levers.get(Lever::Order) > rules.regular,
        window_chance: (style.window_chance * (rules.windows.0 + rules.windows.1 * levers.get(Lever::Wealth))).min(rules.windows.2),
        form: h.form,
        // Interiors affect chimney/stair adaptation and drawing, after placement.
        inside: Vec::new(),
        centre: None,
        seed: rng.next_u64(), row: None, byre,
    };
    Some((spec, index))
}

/// Whether fresh water lies within a few blocks of `at`.
fn wet(ground: &mut Ground, at: Vec2) -> bool {
    let at = at.round().as_ivec2();
    (0..8).any(|k| ground.freshwater(at + (Vec2::from_angle(k as f32 * std::f32::consts::FRAC_PI_4) * 7.0).round().as_ivec2()))
}

/// Tries to place a building of `kind` (or `given`) with its front at `front`, facing
/// `facing`; `lane` is where its path joins the lane.
#[allow(clippy::too_many_arguments)]
fn place(
    town: &mut Town,
    ground: &mut Ground,
    rng: &mut Rng,
    want: &Want,
    given: Option<Spec>,
    front: Vec2,
    facing: Facing,
    occupied: &[Rect],
    lanes: &[Lane],
    lane: Option<Vec2>,
) -> Option<Lot> {
    let (ctx, kit, levers, style) = (town.ctx, town.kit, town.levers, town.style);
    let rules = &ctx.def.rules;
    let kind = match want {
        Want::Kind(kind) => *kind,
        Want::Home | Want::Workshop | Want::Work(_) => Kind::House,
    };
    let mut house = None;
    let mut spec = match (given, kind) {
        (Some(spec), _) => spec,
        (None, Kind::House | Kind::Trader | Kind::Inn) => {
            let near_water = town.needs_water(want) && wet(ground, front);
            let Some((spec, index)) = house_spec(town, want, kind, rng, near_water) else { return miss(0) };
            house = Some(index);
            spec
        }
        (None, kind) => special_spec(kind, 0, 0, kit, levers, rng, style, rules),
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
    // Inside its square of the grid, with room for the ground eased round it.
    if !town.bounds.covers(&rect.grow(FALLOFF + 1)) {
        return town.reject(spec, 1);
    }
    if occupied.iter().any(|o| o.gap(&rect) < town.gap) || town.square.gap(&rect) < 1 {
        return town.reject(spec, 2);
    }
    let ring = if matches!(kind, Kind::Well | Kind::Centrepiece) { RING } else { 0 };
    if lanes.iter().any(|l| l.rect_any(&rect.grow(ring), |d| d < l.half + 0.6)) {
        return town.reject(spec, 3);
    }
    // Firm, gentle ground under it all, and no water under it or on its banks.
    let Some((lowest, highest)) = level(ground, &rect, ctx.def.site.bank, 2) else { return town.reject(spec, 4) };
    let limit = match rules.specials.get(kind.name()) {
        Some(size) => size.slope,
        None => rules.lots.slope + spec.stilts,
    };
    if highest - lowest > limit {
        return town.reject(spec, 5);
    }
    let floor = middle_height(ground, &rect);
    if let Some(at) = lane {
        let road = ground.at(at.round().as_ivec2()).height;
        if (road - floor).abs() > rules.lots.road {
            return town.reject(spec, 6);
        }
    }
    // Its pad is cut level a block round it, but the ground is never cut below the water
    // beside it (`Settlements::apply`): where that water stands over the floor, the ground
    // would stay up against its walls and in its door.
    let pad = rect.grow(1);
    for z in pad.min.y..=pad.max.y {
        for x in pad.min.x..=pad.max.x {
            if ground.at(IVec2::new(x, z)).water > floor {
                return town.reject(spec, 4);
            }
        }
    }
    // Adapt only a placed house. Chimney removal and wall/storey reductions do not
    // change its envelope, stilts, door or any of the placement checks above.
    if let Some(index) = house {
        spec.inside = ctx.def.houses[index].inside.clone();
        house::make_room_for_stairs(&mut spec);
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
    let yard = (rules.lots.yard.0 + ((1.0 - levers.get(Lever::Crowding)) * rules.lots.yard.1) as i32).min(town.gap);
    Some(Lot { spec, rect, frame, floor, mats, door, yard, house, centre: None })
}

/// The world rectangle a building's own box from `low` to `high` covers in `frame`.
fn rect_of(frame: &Frame, low: IVec2, high: IVec2) -> Rect {
    let (a, b) = (frame.at(low.x, 0, low.y), frame.at(high.x, 0, high.y));
    let (a, b) = (IVec2::new(a.x, a.z), IVec2::new(b.x, b.z));
    Rect { min: a.min(b), max: a.max(b) }
}

/// Builds on against the home just placed (the last of `lots`) the rest of a row along its
/// street, on the style's `row` chance (more where the place is crowded): homes wanted next,
/// each the first's twin but for its seed, wall to wall to its right (then its left), on
/// the same floor, while the street runs straight and the ground, the lanes and the lots
/// round it allow. Only a four-walled home off stilts starts one; the row is raised to the
/// storeys it asks of its place's size, the first with it. Returns how far along
/// the street the row runs past the first.
#[allow(clippy::too_many_arguments)]
fn grow_row(town: &mut Town, ground: &mut Ground, lots: &mut Vec<Lot>, occupied: &mut Vec<Rect>, queue: &mut Vec<Want>, lanes: &[Lane], dir: Vec2) -> f32 {
    let Some(row) = town.style.row.clone().filter(|_| town.rows) else { return 0.0 };
    let crowding = town.levers.get(Lever::Crowding);
    let Some(first) = lots.last() else { return 0.0 };
    let spec = &first.spec;
    let seed = spec.seed;
    let fits = spec.kind == Kind::House && spec.form == Form::Box && spec.stilts == 0;
    let along = first.frame.facing.turn(1, 0).as_vec2();
    if !fits || along.dot(dir).abs() < 0.95 || unit(hash3(seed, 0x7044, 0, 0)) >= row.chance * crowding {
        return 0.0;
    }
    // The row's houses: the first's, with nothing out at their sides (no shed, outside
    // stair, woodpile or barrel there) nor a porch before them, under a roof that runs on
    // over a join: along the street (gable ends against each other), gable fronts side by
    // side with their eaves meeting at one height, or flat. Any other is a gable.
    let mut base = spec.clone();
    // As tall as its place's size asks (a town's terraces are of town houses).
    base.storeys = base.storeys.max(row.storeys_at(town.size) as i32);
    base.extras.retain(|e| !matches!(e, Extra::Porch | Extra::OutsideStair | Extra::Woodpile | Extra::RainBarrel));
    base.shed = 0;
    let runs_on = matches!(base.roof, Roof::Gable | Roof::Gambrel | Roof::Flat) || (base.roof == Roof::Saltbox && base.ridge_x);
    if !runs_on {
        base.roof = Roof::Gable;
    }
    house::make_room_for_stairs(&mut base);
    let wanted = row.houses.0 + (unit(hash3(seed, 0x7044, 1, 0)) * (row.houses.1 - row.houses.0 + 1) as f32) as u32;
    let wanted = wanted.min(row.houses.1);
    let ctx = town.ctx;
    let rules = &ctx.def.rules;
    let index = lots.len() - 1;
    // The lots round the row, not its own.
    let others = occupied.len() - lots[index].taken().count();
    let (floor, mats, door, yard, house) = {
        let l = &lots[index];
        (l.floor, l.mats.clone(), l.door, l.yard, l.house)
    };
    // How far the first's door stands from the street, for its row to keep to.
    let first_off = {
        let outside = lots[index].frame.at(base.door, 0, -2);
        let outside = Vec2::new(outside.x as f32, outside.z as f32);
        lanes.iter().map(|l| l.distance(outside)).fold(f32::MAX, f32::min)
    };
    // Built on to the right while it can be, then to the left of the first.
    let mut built = 1;
    let mut ends = [index, index];
    for side in [1i32, -1] {
        while built < wanted {
            if !queue.iter().any(|w| matches!(w, Want::Home)) {
                break;
            }
            let end = ends[(side < 0) as usize];
            let prev = &lots[end];
            if let Some(i) = prev.house
                && !house_fits(&ctx.def.houses[i], &Want::Home, town.size, &town.style.name, town.built.get(&i).copied().unwrap_or(0), rules.houses.grand)
            {
                break;
            }
            let mut next = base.clone();
            next.seed = hash3(seed, 0x7045, built as i32, 0);
            next.row = Some(house::Row { seed, left: side > 0, right: side < 0 });
            let origin = prev.frame.at(if side > 0 { prev.spec.w } else { -base.w }, 0, 0);
            let frame = Frame { origin, facing: prev.frame.facing };
            let (low, high) = next.envelope();
            let rect = rect_of(&frame, low, high);
            if !town.bounds.covers(&rect.grow(FALLOFF + 1)) || occupied[..others].iter().any(|o| o.gap(&rect) < town.gap) || town.square.gap(&rect) < 1 {
                break;
            }
            if lanes.iter().any(|l| l.rect_any(&rect, |d| d < l.half + 0.6)) {
                break;
            }
            let Some((lowest, highest)) = level(ground, &rect, ctx.def.site.bank, 2) else { break };
            if highest - lowest > rules.lots.slope || (middle_height(ground, &rect) - floor).abs() > 1 {
                break;
            }
            let pad = rect.grow(1);
            if (pad.min.y..=pad.max.y).any(|z| (pad.min.x..=pad.max.x).any(|x| ground.at(IVec2::new(x, z)).water > floor)) {
                break;
            }
            let outside = frame.at(next.door, 0, -2);
            let outside = IVec2::new(outside.x, outside.z);
            // Its path runs out to the street where the first's does, moved along it: on the
            // lane still, and its door no farther from the lane than the first's (two blocks
            // more at most), or the row ends here: a lane that bends away, or one a little
            // off square that the row's square steps would walk away from.
            let to = door.map(|(d, to)| to + dir * (outside - d).as_vec2().dot(dir)).unwrap_or(outside.as_vec2());
            let off = |p: Vec2| lanes.iter().map(|l| l.distance(p)).fold(f32::MAX, f32::min);
            if !lanes.iter().any(|l| l.distance(to) <= l.half) || off(outside.as_vec2()) > first_off + 2.0 || (ground.at(to.round().as_ivec2()).height - floor).abs() > rules.lots.road {
                break;
            }
            // The end it is built on against: no eave or window on that side now.
            let mut joined = if end == index && lots[end].spec.row.is_none() { base.clone() } else { lots[end].spec.clone() };
            let mut place = joined.row.unwrap_or(house::Row { seed, left: false, right: false });
            if side > 0 { place.right = true } else { place.left = true }
            joined.row = Some(place);
            let (pl, ph) = joined.envelope();
            let shrunk = rect_of(&lots[end].frame, pl, ph);
            if let Some(o) = occupied.iter_mut().rev().find(|o| **o == lots[end].rect) {
                *o = shrunk;
            }
            lots[end].spec = joined;
            lots[end].rect = shrunk;
            let lot = Lot { spec: next, rect, frame, floor, mats: mats.clone(), door: Some((outside, to)), yard, house, centre: None };
            town.count(&lot);
            occupied.extend(lot.taken());
            lots.push(lot);
            ends[(side < 0) as usize] = lots.len() - 1;
            if let Some(k) = queue.iter().position(|w| matches!(w, Want::Home)) {
                queue.remove(k);
            }
            built += 1;
        }
    }
    // How far past the first, along the street, the row runs on.
    let past = |end: usize| {
        let d = (lots[end].frame.origin - lots[index].frame.origin).as_vec3();
        Vec2::new(d.x, d.z).dot(dir)
    };
    past(ends[0]).max(past(ends[1])).max(0.0)
}

/// The lowest and highest ground under `rect`, its height sampled every `step` blocks and
/// along its edges; none where any of it is not solid ground out of the water's channel, or
/// water lies within `bank` blocks of it. Water is looked for at every block, under it and
/// on its banks alike, as a creek one block wide could lie between samples.
fn level(ground: &mut Ground, rect: &Rect, bank: i32, step: i32) -> Option<(i32, i32)> {
    #[cfg(test)]
    { ground.levels += 1; }
    #[cfg(test)]
    if ORIGINAL_ROWS.with(|flag| flag.get()) { return level_original(ground, rect, bank, step); }
    let outer = rect.grow(bank);
    let (mut lowest, mut highest) = (i32::MAX, i32::MIN);
    let step = step.max(1);
    let sampled = |v: i32, lo: i32, hi: i32| (v - lo) % step == 0 || v == hi;
    let mut left = 0;
    let mut values = [None; crate::water::SAMPLE_BATCH];
    let mut consumed = 0;
    for z in outer.min.y..=outer.max.y {
        for x in outer.min.x..=outer.max.x {
            if left == 0 {
                let mut points = [IVec2::ZERO; crate::water::SAMPLE_BATCH];
                let mut next = IVec2::new(x, z);
                for point in &mut points {
                    *point = next;
                    left += 1;
                    if next.x < outer.max.x { next.x += 1; }
                    else if next.y < outer.max.y { next = IVec2::new(outer.min.x, next.y + 1); }
                    else { break; }
                }
                values = ground.prefetch_unique(&points[..left]);
                consumed = 0;
            }
            left -= 1;
            let p = IVec2::new(x, z);
            #[cfg(test)]
            { ground.queries += 1; }
            let g = values[consumed].expect("prefetched terrain facts");
            consumed += 1;
            if g.submerged { return None; }
            if rect.holds(p) {
                if g.river >= 0.5 { return None; }
                if sampled(x, outer.min.x, outer.max.x) && sampled(z, outer.min.y, outer.max.y) {
                    if !g.solid { return None; }
                    lowest = lowest.min(g.height);
                    highest = highest.max(g.height);
                }
            }
        }
    }
    (lowest <= highest).then_some((lowest, highest))
}

/// Literal original leveling loop, independent of the compact terrain projection.
#[cfg(test)]
fn level_original(ground: &mut Ground, rect: &Rect, bank: i32, step: i32) -> Option<(i32, i32)> {
    let outer = rect.grow(bank);
    let (mut lowest, mut highest) = (i32::MAX, i32::MIN);
    let step = step.max(1);
    let sampled = |v: i32, lo: i32, hi: i32| (v - lo) % step == 0 || v == hi;
    for z in outer.min.y..=outer.max.y {
        for x in outer.min.x..=outer.max.x {
            if (x - outer.min.x) % crate::water::SAMPLE_BATCH as i32 == 0 {
                ground.prefetch(IVec2::new(x, z), (outer.max.x - x + 1).min(crate::water::SAMPLE_BATCH as i32) as usize);
            }
            let p = IVec2::new(x, z);
            let g = ground.at(p);
            if g.submerged() {
                return None;
            }
            if rect.holds(p) {
                if g.river >= 0.5 {
                    return None;
                }
                if sampled(x, outer.min.x, outer.max.x) && sampled(z, outer.min.y, outer.max.y) {
                    if !solid(&g) {
                        return None;
                    }
                    lowest = lowest.min(g.height);
                    highest = highest.max(g.height);
                }
            }
        }
    }
    (lowest <= highest).then_some((lowest, highest))
}

/// The middle height of the ground under `rect`, from a few samples.
fn middle_height(ground: &mut Ground, rect: &Rect) -> i32 {
    let size = rect.max - rect.min + 1;
    let step = IVec2::new((size.x - 1).max(1) / 3, (size.y - 1).max(1) / 3).max(IVec2::ONE);
    let mut heights = Vec::with_capacity(16);
    let mut z = rect.min.y;
    while z <= rect.max.y {
        let mut x = rect.min.x;
        while x <= rect.max.x {
            heights.push(ground.at(IVec2::new(x, z)).height);
            x += step.x;
        }
        z += step.y;
    }
    heights.sort_unstable();
    heights[heights.len() / 2]
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
            let block = fixed.crates.pick(u(0xc7)).unwrap_or(mats.trim);
            put(0, 1, 0, Put::Soft(block));
            if u(3) < 0.4 {
                put(0, 2, 0, Put::Soft(block));
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
            // Sunk a block into level ground (the plan pads it), so its water stays put.
            put(0, -1, 0, Put::Set(fixed.water));
            put(0, -2, 0, Put::Set(mats.stone()));
            put(0, 0, 0, Put::Clear);
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
            if let Some(net) = fixed.nets.pick(u(0x4e)) {
                put(1, 2, 0, Put::Set(net));
            }
        }
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;
    use crate::geology::GeoSample;
    use crate::trails::Land;
    use crate::water::Body;

    #[test]
    fn terrain_batches_keep_mixed_level_facts_and_original_predicates() {
        struct Mixed(Vec<(IVec2, GeoSample)>);
        impl Land for Mixed {
            fn sample(&self, x: i32, z: i32) -> GeoSample {
                self.0.iter().find(|(p, _)| *p == IVec2::new(x, z))
                    .map(|(_, g)| *g).unwrap_or_else(|| Creek(None, None).sample(x, z))
            }
        }
        let dry = Creek(None, None).sample(0, 0);
        let shape = GeoSample { form: crate::geology::Form { shape: Some(crate::ShapeId(0)), ..dry.form }, ..dry };
        let cavity = GeoSample { cavity: Some(crate::water::Cavity { floor: 130, ceiling: 139 }), ..dry };
        let below = GeoSample { river: 0.5f32.next_down(), ..dry };
        let equal = GeoSample { river: 0.5, ..dry };
        let above = GeoSample { river: 0.5f32.next_up(), ..dry };
        let nan = GeoSample { river: f32::from_bits(0x7fc0_0123), ..dry };
        let wet = GeoSample { water: dry.height + 1, ..dry };
        let samples = [dry, shape, cavity, below, equal, above, nan, wet];
        let points: Vec<_> = (0..samples.len()).map(|i| IVec2::new(i as i32, -17)).collect();
        let land = Mixed(points.iter().copied().zip(samples).collect());
        for warm in [0, 2, samples.len()] {
            let mut ground = Ground::new(&land);
            for (&p, &g) in points.iter().zip(&samples).take(warm) { ground.seen.insert(p, g); }
            let values = ground.prefetch_unique(&points);
            for (i, g) in samples.iter().enumerate() {
                let v = values[i].expect("actual sample in every supplied slot");
                assert_eq!((v.height, v.river.to_bits(), v.submerged, v.solid),
                    (g.height, g.river.to_bits(), g.water > g.height, g.form.shape.is_none() && g.cavity.is_none()));
            }
            assert!(values[points.len()..].iter().all(Option::is_none));
            assert_eq!(ground.prefetched, samples.len() - warm);
        }

        let rect = Rect::new(IVec2::ZERO, IVec2::splat(5));
        let bank = IVec2::new(-1, 0);
        let unsampled = IVec2::ZERO;
        let sampled = IVec2::ONE;
        for (p, g, accepted) in [
            (bank, GeoSample { river: above.river, ..shape }, true),
            (bank, GeoSample { river: above.river, ..cavity }, true),
            (unsampled, GeoSample { height: 9999, water: 9999, ..shape }, true),
            (unsampled, cavity, true),
            (sampled, shape, false),
            (sampled, cavity, false),
            (sampled, below, true),
            (unsampled, equal, false),
            (unsampled, above, false),
            (sampled, nan, true),
            (bank, dry, true),
            (bank, wet, false),
            (unsampled, wet, false),
        ] {
            let land = Mixed(vec![(p, g)]);
            let outer = rect.grow(1);
            let points: Vec<_> = (outer.min.y..=outer.max.y).flat_map(|z|
                (outer.min.x..=outer.max.x).map(move |x| IVec2::new(x, z))).collect();
            for warm in [0, points.len() / 2, points.len()] {
                let (mut original, mut compact) = (Ground::new(&land), Ground::new(&land));
                for &p in points.iter().take(warm) {
                    let g = land.sample(p.x, p.y);
                    original.seen.insert(p, g);
                    compact.seen.insert(p, g);
                }
                let expected = with_original_rows(|| level(&mut original, &rect, 1, 2));
                assert_eq!(expected, accepted.then_some((140, 140)), "literal original fixture at {p:?}");
                assert_eq!(level(&mut compact, &rect, 1, 2), expected);
                assert_eq!(compact.queries, original.queries, "same first rejection at {p:?}");
                assert_eq!((compact.levels, original.levels), (1, 1));
            }
        }
    }

    #[test]
    fn terrain_batches_keep_original_leveling_and_reduce_calls() {
        let (compact, full) = (std::mem::size_of::<[Option<LevelSample>; 32]>(), std::mem::size_of::<[Option<GeoSample>; 32]>());
        assert!(compact < full, "the returned batch holds only leveling's actual facts");
        println!("level value buffers: {compact} compact bytes versus {full} full-sample bytes");
        struct Batches {
            creek: Creek,
            calls: std::sync::Mutex<Vec<Vec<(i32, i32)>>>,
        }
        impl Land for Batches {
            fn sample(&self, x: i32, z: i32) -> GeoSample { self.creek.sample(x, z) }
            fn samples_cached(&self, points: &[(i32, i32)], _: &mut crate::water::Samples) -> Vec<GeoSample> {
                self.calls.lock().unwrap().push(points.to_vec());
                points.iter().map(|&(x, z)| self.sample(x, z)).collect()
            }
        }
        let mut reduced = 0;
        let mut accepted = 0;
        let mut rejected = 0;
        for offset in [IVec2::ZERO, IVec2::new(-113, -97), IVec2::new(65_537, -65_535)] {
            for size in [IVec2::new(3, 4), IVec2::new(20, 17), IVec2::new(65, 5)] {
                let rect = Rect::new(offset, size);
                let outer = rect.grow(3);
                for creek in [None, Some(outer.min.x), Some(rect.min.x + 1), Some(rect.max.x)] {
                    let original_land = Batches { creek: Creek(creek, None), calls: Default::default() };
                    let batched_land = Batches { creek: Creek(creek, None), calls: Default::default() };
                    let (mut original, mut batched) = (Ground::new(&original_land), Ground::new(&batched_land));
                    let expected = with_original_rows(|| level(&mut original, &rect, 3, 2));
                    assert_eq!(level(&mut batched, &rect, 3, 2), expected);
                    assert_eq!(batched.queries, original.queries, "original row-major rejection point");
                    assert!(batched.seen.len() + batched.dry_water.len() <= GROUND_SAMPLE_LIMIT);
                    let calls = batched_land.calls.lock().unwrap();
                    assert!(calls.iter().all(|points| points.len() <= crate::water::SAMPLE_BATCH));
                    let actual: Vec<_> = calls.iter().flatten().copied().collect();
                    let ordered: Vec<_> = (outer.min.y..=outer.max.y).flat_map(|z|
                        (outer.min.x..=outer.max.x).map(move |x| (x, z))).collect();
                    assert_eq!(actual, ordered[..actual.len()], "bounded lookahead keeps exact original point order");
                    assert_eq!(batched.prefetched, actual.len());
                    if expected.is_some() {
                        accepted += 1;
                        assert_eq!(actual, ordered);
                        assert_eq!(batched.cache_reads, ordered.len(), "consume each prefetched value without another lookup");
                        assert_eq!(original.cache_reads, ordered.len() * 2, "original prefetch plus consumed readback");
                        assert_eq!(calls.len(), ordered.len().div_ceil(crate::water::SAMPLE_BATCH));
                        reduced += usize::from(calls.len() < original_land.calls.lock().unwrap().len());
                    } else {
                        rejected += 1;
                    }
                }
            }
        }
        assert!(accepted > 0 && rejected > 0 && reduced > 0);

        let land = Batches { creek: Creek(Some(-3), None), calls: Default::default() };
        let points: Vec<_> = (0..7).map(|x| IVec2::new(x - 3, -17)).collect();
        let mut warm = Ground::new(&land);
        for &i in &[1, 3] {
            let p = points[i];
            warm.seen.insert(p, land.sample(p.x, p.y));
        }
        warm.dry_water.insert(points[2]);
        let values = warm.prefetch_unique(&points);
        let missing: Vec<_> = points.iter().enumerate().filter(|(i, _)| ![1, 3].contains(i))
            .map(|(_, p)| (p.x, p.y)).collect();
        assert_eq!(*land.calls.lock().unwrap(), vec![missing.clone()], "partially warm misses retain order");
        assert_eq!(warm.prefetched, missing.len());
        assert!(!warm.dry_water.contains(&points[2]), "a full sample replaces its dry witness");
        for (i, &p) in points.iter().enumerate() {
            let g = land.sample(p.x, p.y);
            assert_eq!(values[i], Some(LevelSample { height: g.height, river: g.river, submerged: g.submerged(), solid: g.form.shape.is_none() && g.cavity.is_none() }), "scatter cold and warm facts into original slots");
            assert_eq!(warm.at(p), land.sample(p.x, p.y));
        }
        assert!(values[points.len()..].iter().all(Option::is_none));
        assert_eq!(warm.prefetch_unique(&points), values);
        assert_eq!(land.calls.lock().unwrap().len(), 1, "a fully warm batch reads no terrain");
        assert!(warm.prefetch_unique(&[]).iter().all(Option::is_none));
        assert_eq!(land.calls.lock().unwrap().len(), 1, "an empty batch reads no terrain");

        land.calls.lock().unwrap().clear();
        let mut pressured = Ground::new(&land);
        let first = points[0];
        pressured.seen.insert(first, GeoSample { height: 9999, ..land.sample(first.x, first.y) });
        pressured.dry_water.extend((0..GROUND_SAMPLE_LIMIT - 1).map(|i| IVec2::new(100_000 + i as i32, 100_000)));
        assert_eq!(pressured.seen.len() + pressured.dry_water.len(), GROUND_SAMPLE_LIMIT);
        let values = pressured.prefetch_unique(&points);
        let all: Vec<_> = points.iter().map(|p| (p.x, p.y)).collect();
        assert_eq!(*land.calls.lock().unwrap(), vec![all], "clear before collecting even the previously cached first point");
        assert_eq!(pressured.clears, 1);
        assert_eq!(pressured.prefetched, points.len());
        assert_eq!(pressured.seen.len(), points.len());
        assert!(pressured.dry_water.is_empty());
        for (i, &p) in points.iter().enumerate() {
            let g = land.sample(p.x, p.y);
            assert_eq!(values[i], Some(LevelSample { height: g.height, river: g.river, submerged: g.submerged(), solid: g.form.shape.is_none() && g.cavity.is_none() }), "clearing precedes copying any warm fact");
            assert_eq!(pressured.at(p), land.sample(p.x, p.y));
        }
        assert!(values[points.len()..].iter().all(Option::is_none));

        land.calls.lock().unwrap().clear();
        let mut all_warm = Ground::new(&land);
        for &p in &points { all_warm.seen.insert(p, GeoSample { height: 9999, ..land.sample(p.x, p.y) }); }
        all_warm.dry_water.extend((0..GROUND_SAMPLE_LIMIT - points.len()).map(|i| IVec2::new(100_000 + i as i32, 100_000)));
        let values = all_warm.prefetch_unique(&points);
        assert_eq!(all_warm.clears, 1, "even a fully warm batch reserves before any lookup");
        assert_eq!(all_warm.prefetched, points.len());
        assert_eq!(*land.calls.lock().unwrap(), vec![points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()]);
        for (i, &p) in points.iter().enumerate() {
            let g = land.sample(p.x, p.y);
            assert_eq!(values[i], Some(LevelSample { height: g.height, river: g.river, submerged: g.submerged(), solid: g.form.shape.is_none() && g.cavity.is_none() }));
        }
        assert!(values[points.len()..].iter().all(Option::is_none));
    }

    #[test]
    fn lane_step_bounds_keep_original_signed_clearance() {
        let mut near = 0;
        let mut far = 0;
        for offset in [0.0, -8192.0, 1_048_576.0, 16_777_216.0, -16_777_216.0] {
            let shift = Vec2::new(offset, -offset);
            for (a, b) in [
                (Vec2::new(-10.0, 0.0), Vec2::new(10.0, 0.0)),
                (Vec2::new(0.0, -10.0), Vec2::new(0.0, 10.0)),
                (Vec2::new(-0.25, 0.5), Vec2::new(0.75, -0.25)),
                (Vec2::splat(50.0), Vec2::splat(100.0)),
                (Vec2::ONE, Vec2::ONE),
            ] {
                for rect in [Rect::new(shift.as_ivec2(), IVec2::splat(3)), Rect::new(shift.as_ivec2() - 8, IVec2::new(12, 7))] {
                    let (a, b) = (a + shift, b + shift);
                    let minimum = (0..=6).map(|k| rect.signed(a.lerp(b, k as f32 / 6.0))).fold(f32::MAX, f32::min);
                    let step = StepClearance::new(a, b);
                    for clear in [0.0, 0.5, 1.0, 3.0, 10.0, minimum.next_down(), minimum, minimum.next_up(), f32::NEG_INFINITY, f32::INFINITY, f32::NAN] {
                        let original = minimum < clear.min(rect.signed(a) - 0.01);
                        assert_eq!(step.nears(&rect, clear), original, "{a:?}/{b:?}/{rect:?}/{clear}");
                        near += usize::from(original);
                        far += usize::from(!original);
                    }
                }
            }
        }
        let rect = Rect::new(IVec2::ZERO, IVec2::ONE);
        // Finite points can have overflowing distances, capped by the original
        // MAX minimum; non-finite interpolation must retain the full calculation.
        for (a, b) in [(Vec2::splat(1e30), Vec2::splat(2e30)), (Vec2::splat(f32::MAX), Vec2::splat(-f32::MAX)), (Vec2::splat(f32::NAN), Vec2::ZERO)] {
            let step = StepClearance::new(a, b);
            for clear in [1.0, f32::MAX, f32::INFINITY, f32::NAN] {
                let original = (0..=6).map(|k| rect.signed(a.lerp(b, k as f32 / 6.0))).fold(f32::MAX, f32::min) < clear.min(rect.signed(a) - 0.01);
                assert_eq!(step.nears(&rect, clear), original);
            }
        }
        assert!(near > 0 && far > 0, "actual matching and excluded steps must occur");
    }

    #[test]
    fn house_choices_keep_original_order_weights_and_draws() {
        let mut def = SettlementsDef::default();
        def.extend("base", "base", include_str!("../../../../assets/content/base/settlements.ron")).unwrap();
        // Include modded water-only homes and a zero-weight final entry. The
        // original selector can fall through to a final zero after rounding.
        let mut boat = def.houses.iter().find(|h| h.name == "boathouse").unwrap().clone();
        boat.work = None;
        boat.width.1 = def.rules.houses.grand;
        def.houses.push(boat);
        let wants = [Want::Home, Want::Workshop, Want::Kind(Kind::Inn), Want::Kind(Kind::Trader), Want::Work("shrine".into()), Want::Work("boathouse".into()), Want::Work("absent".into())];
        let mut wet_only = 0;
        let mut empty = 0;
        let mut capped = 0;
        let mut style_excluded = 0;
        for all_zero in [false, true] {
            let weights: Vec<_> = (0..def.houses.len()).map(|i| if all_zero || i % 3 == 0 || i + 1 == def.houses.len() { 0.0 } else { 0.25 + i as f32 * 0.125 }).collect();
            for size in ["hamlet", "village", "town"] {
                for style in ["timber_frame", "log_cabin", "turf_house"] {
                    assert!(def.styles.iter().any(|s| s.name == style));
                    for count in 0..=3 {
                        let built: FxHashMap<_, _> = (0..def.houses.len()).map(|i| (i, count)).collect();
                        for want in &wants {
                            let choices = HouseChoices::new(&def.houses, &weights, want, size, style, &built, def.rules.houses.grand);
                            let eligible = |h: &super::super::def::HouseDef| {
                                (h.sizes.is_empty() || h.sizes.iter().any(|s| s == size))
                                    && !h.not_in.iter().any(|s| s == style)
                                    && h.most.is_none_or(|most| count < most)
                                    && match want {
                                        Want::Kind(Kind::Inn) => h.work.is_none() && h.form == Form::Box && h.width.1 >= def.rules.houses.grand,
                                        Want::Kind(Kind::Trader) => h.work.is_none() && matches!(h.form, Form::Box | Form::ShopFront),
                                        Want::Home | Want::Kind(_) => h.work.is_none(),
                                        Want::Workshop => h.work.is_some(),
                                        Want::Work(work) => h.work.as_deref() == Some(work.as_str()),
                                    }
                            };
                            assert_eq!(choices.needs_water, def.houses.iter().any(|h| h.water && eligible(h)));
                            wet_only += usize::from(choices.needs_water);
                            capped += def.houses.iter().filter(|h| h.most.is_some_and(|most| count >= most)).count();
                            style_excluded += def.houses.iter().filter(|h| h.not_in.iter().any(|s| s == style)
                                && house_fits(h, want, size, "unexcluded", count, def.rules.houses.grand)).count();
                            for near_water in [false, true] {
                                let original: Vec<_> = def.houses.iter().enumerate().filter(|(_, h)| eligible(h) && (!h.water || near_water)).map(|(i, _)| (i, weights[i])).collect();
                                let cached = if near_water { &choices.wet } else { &choices.dry };
                                assert_eq!(cached, &original, "{size}/{style}/{count}/{want:?}/{near_water}");
                                empty += usize::from(original.is_empty());
                                let (mut a, mut b) = (Rng::new(42, 71, -19, 23), Rng::new(42, 71, -19, 23));
                                for _ in 0..32 { assert_eq!(pick(&mut a, &original), pick(&mut b, cached)); }
                                assert_eq!(a.next_u64(), b.next_u64(), "selection consumes identical draws even when no weight is positive");
                            }
                        }
                    }
                }
            }
        }
        assert!(wet_only > 0 && empty > 0 && capped > 0 && style_excluded > 0, "water, missing requests, exhausted caps and actual style exclusions are exercised");
    }

    #[test]
    fn rejected_house_extras_keep_original_rolls_and_owned_specs() {
        let world = super::super::tests::base_world(7);
        let def = &world.settlements.def;
        let land = Creek(None, None);
        let ctx = Ctx { world: &world, land: &land, def, resolved: &world.settlements.resolved,
            fixed: world.settlements.fixed.as_ref().unwrap(), seed: world.settlements.seed,
            spawn: None, promised: &[], cells: &world.settlements.cells };
        let style = def.styles.iter().find(|s| s.name == "timber_frame").unwrap();
        let levers = Levers::default();
        let kit = Kit { wood: String::new(), log: String::new(), foundation: Palette(Vec::new()),
            floor: Vec::new(), walls: Vec::new(), frame: Vec::new(), roof: Vec::new(), trims: Vec::new(),
            paving: Palette(Vec::new()), hedge: Palette(Vec::new()), weathered: Palette(Vec::new()),
            chimney: Vec::new(), crops: Vec::new(), fruit: Palette(Vec::new()), roofs: Vec::new(), window: None, roof_pieces: Vec::new(), pieced: 0.0, jetty: None };
        let rect = Rect::new(IVec2::ZERO, IVec2::ONE);
        let new = || Town { ctx: &ctx, kit: &kit, levers: &levers, house_weights: vec![1.0; def.houses.len()],
            extra_chances: vec![0.5; def.extras.len()], style, size: "village", built: FxHashMap::default(),
            choices: Vec::new(), spare_extras: Vec::new(), bounds: rect, square: rect, gap: 1, rows: true };
        let (mut reused, mut fresh) = (new(), new());
        reused.spare_extras = Vec::with_capacity(32);
        reused.spare_extras.extend([Extra::Chimney; 32]);
        let (mut a, mut b) = (Rng::new(7, 71, -19, 23), Rng::new(7, 71, -19, 23));
        let mut extras = 0;
        for want in [Want::Home, Want::Workshop, Want::Kind(Kind::Trader), Want::Kind(Kind::Inn), Want::Work("absent".into())] {
            let kind = match want { Want::Kind(kind) => kind, _ => Kind::House };
            for _ in 0..32 {
                let actual = house_spec(&mut reused, &want, kind, &mut a, true);
                let original = house_spec(&mut fresh, &want, kind, &mut b, true);
                assert_eq!(format!("{actual:?}"), format!("{original:?}"), "poisoned reusable storage must keep every original spec field and extra in order");
                if let Some((spec, _)) = actual {
                    extras += spec.extras.len();
                    let pointer = spec.extras.as_ptr();
                    let capacity = spec.extras.capacity();
                    assert!(reused.reject(spec, 1).is_none());
                    assert_eq!(reused.spare_extras.as_ptr(), pointer);
                    assert_eq!(reused.spare_extras.capacity(), capacity);
                }
                // The reference discards each allocation, as the original path did.
                drop(original);
                assert_eq!(a.next_u64(), b.next_u64());
            }
        }
        assert!(extras > 0);
        // Exercise place's actual bounds-rejection route after the same house roll.
        let original = house_spec(&mut fresh, &Want::Home, Kind::House, &mut b, false).unwrap();
        let pointer = reused.spare_extras.as_ptr();
        assert!(place(&mut reused, &mut Ground::new(&land), &mut a, &Want::Home, None,
            Vec2::splat(1000.0), Facing::North, &[], &[], None).is_none());
        assert_eq!(reused.spare_extras.as_ptr(), pointer);
        assert_eq!(a.next_u64(), b.next_u64());
        drop(original);
        // A retained spec owns its extras; rolling another cannot overwrite it.
        let (retained, _) = house_spec(&mut reused, &Want::Home, Kind::House, &mut a, false).unwrap();
        let expected = retained.extras.clone();
        let next = house_spec(&mut reused, &Want::Home, Kind::House, &mut a, false).unwrap();
        assert_eq!(retained.extras, expected);
        assert!(reused.spare_extras.is_empty());
        drop(next);
    }

    #[test]
    fn counted_house_invalidates_every_warm_choice() {
        let world = super::super::tests::base_world(7);
        let def = &world.settlements.def;
        let land = Creek(None, None);
        let ctx = Ctx { world: &world, land: &land, def, resolved: &world.settlements.resolved,
            fixed: world.settlements.fixed.as_ref().unwrap(), seed: world.settlements.seed,
            spawn: None, promised: &[], cells: &world.settlements.cells };
        let style = def.styles.iter().find(|s| s.name == "timber_frame").unwrap();
        let levers = Levers::default();
        // Selection and counting need no painted materials for this fixture.
        let kit = Kit { wood: String::new(), log: String::new(), foundation: Palette(Vec::new()),
            floor: Vec::new(), walls: Vec::new(), frame: Vec::new(), roof: Vec::new(), trims: Vec::new(),
            paving: Palette(Vec::new()), hedge: Palette(Vec::new()), weathered: Palette(Vec::new()),
            chimney: Vec::new(), crops: Vec::new(), fruit: Palette(Vec::new()), roofs: Vec::new(), window: None, roof_pieces: Vec::new(), pieced: 0.0, jetty: None };
        let rect = Rect::new(IVec2::ZERO, IVec2::ONE);
        let mut town = Town { ctx: &ctx, kit: &kit, levers: &levers, house_weights: vec![1.0; def.houses.len()],
            extra_chances: vec![0.0; def.extras.len()], style, size: "village", built: FxHashMap::default(), choices: Vec::new(), spare_extras: Vec::new(), bounds: rect, square: rect, gap: 1, rows: true };
        let want = Want::Work("boathouse".into());
        let index = def.houses.iter().position(|h| h.name == "boathouse").unwrap();
        assert_eq!(def.houses[index].most, Some(1));
        for request in [&want, &Want::Workshop] {
            assert!(town.choices(request).wet.iter().any(|&(i, _)| i == index));
        }
        assert_eq!(town.choices.len(), 2);
        let mut rng = Rng::new(7, 71, -19, 23);
        let (spec, chosen) = house_spec(&mut town, &want, Kind::House, &mut rng, true).unwrap();
        assert_eq!(chosen, index);
        assert_eq!(town.choices.len(), 2, "a selection attempt leaves both warm requests available");
        let frame = Frame::fit(IVec2::ZERO, IVec2::ONE, Facing::North, IVec2::ZERO, 140);
        let lot = Lot { spec, rect, frame, floor: 140, mats: kit.mats(&ctx, &mut rng), door: None, yard: 0, house: Some(index), centre: None };
        town.count(&lot);
        assert_eq!(town.built[&index], 1);
        assert!(town.choices.is_empty(), "counting invalidates all requests, including those not used to pick this lot");
        for request in [&want, &Want::Workshop] {
            let choices = town.choices(request);
            assert!(!choices.wet.iter().chain(&choices.dry).any(|&(i, _)| i == index));
        }
        assert!(!town.needs_water(&want), "the exhausted water-only request no longer asks for a water search");
    }

    #[test]
    fn required_house_preflight_only_rejects_impossible_catalogues() {
        let mut def = SettlementsDef::default();
        def.extend("base", "base", include_str!("../../../../assets/content/base/settlements.ron")).unwrap();
        let works = [Want::Work("shrine".into())];
        let mixed = [Want::Kind(Kind::Inn), Want::Kind(Kind::Trader), Want::Kind(Kind::Smithy), Want::Work("shrine".into()), Want::Home, Want::Workshop];
        let possible = |houses: &[super::super::def::HouseDef], weights: &[f32], queue: &[Want], size, style| {
            required_houses_possible(houses, weights, queue, size, style, def.rules.houses.grand)
        };
        let mut weights = vec![1.0; def.houses.len()];
        assert!(possible(&def.houses, &weights, &mixed, "village", "timber_frame"));
        assert!(!possible(&def.houses, &weights, &[Want::Work("absent_work".into())], "village", "timber_frame"));
        assert!(!possible(&def.houses, &weights, &works, "hamlet", "timber_frame"), "shrine catalogue is village-only");
        assert!(possible(&def.houses, &weights, &[Want::Work("boathouse".into())], "village", "timber_frame"), "water-only entries remain possible without a water witness");
        weights.fill(0.0);
        assert!(!possible(&def.houses, &weights, &mixed, "village", "timber_frame"));
        assert!(possible(&def.houses, &weights, &[Want::Kind(Kind::Smithy), Want::Home, Want::Workshop], "village", "timber_frame"), "specials and optional houses are outside this check");
        let chapel = def.houses.iter().position(|h| h.work.as_deref() == Some("shrine")).unwrap();
        weights[chapel] = 1.0;
        assert!(possible(&def.houses, &weights, &works, "village", "timber_frame"), "zero-weight neighbours never hide a selectable entry");
        assert!(possible(&def.houses, &weights, &[works[0].clone(), works[0].clone()], "village", "timber_frame"), "no speculative total-capacity rejection");
        assert!(!possible(&def.houses, &weights, &mixed, "village", "timber_frame"), "an available shrine cannot satisfy a missing inn or trader");
        def.houses[chapel].most = Some(0);
        assert!(!possible(&def.houses, &weights, &works, "village", "timber_frame"));
        def.houses[chapel].most = None;
        def.houses[chapel].not_in.push("timber_frame".into());
        assert!(!possible(&def.houses, &weights, &works, "village", "timber_frame"));
        assert!(possible(&def.houses, &weights, &works, "village", "log_house"));
    }

    #[test]
    fn lane_turn_batches_keep_scalar_paths_and_rng() {
        // The original scalar walker is the oracle for float ties and RNG draws.
        #[allow(clippy::too_many_arguments)]
        fn scalar(ground: &mut Ground, lanes: &[Lane], rng: &mut Rng, from: Vec2, angle: f32, length: f32, wiggle: f32, bounds: &Rect, rules: &LanesDef, avoid: &[(Rect, f32)]) -> Vec<Vec2> {
            let step = rules.step;
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
                    let next = here + Vec2::from_angle(a) * step;
                    let at = next.round().as_ivec2();
                    let nears = |r: &Rect, clear: f32| (0..=6).map(|k| r.signed(here.lerp(next, k as f32 / 6.0))).fold(f32::MAX, f32::min) < clear.min(r.signed(here) - 0.01);
                    if !bounds.holds(at) || avoid.iter().any(|(r, clear)| nears(r, *clear)) { continue; }
                    let g = ground.at(at);
                    if !walkable(&g) { continue; }
                    let rise = (g.height - height).abs();
                    if rise > rules.rise { continue; }
                    if walked > rules.leave && lanes.iter().any(|l| l.distance(next) < l.half + rules.apart) { continue; }
                    let cost = rise as f32 * 1.5 + turn.abs() * 1.2;
                    if best.is_none_or(|b| cost < b.0) { best = Some((cost, a, next, g.height)); }
                }
                let Some((_, a, next, h)) = best else { break };
                heading = a;
                here = next;
                height = h;
                points.push(here);
                walked += step;
            }
            points
        }
        struct WalkingLand(Creek, bool);
        impl Land for WalkingLand {
            fn sample(&self, x: i32, z: i32) -> GeoSample {
                let mut g = self.0.sample(x, z);
                if self.1 && !g.submerged() {
                    g.height += x.rem_euclid(17) / 2;
                    if z.rem_euclid(19) == 0 { g.crag.amplitude = 1.0; }
                }
                g
            }
        }
        let rules = SettlementsDef::default().rules.lanes;
        let (mut long, mut short) = (0, 0);
        for centre in [IVec2::ZERO, IVec2::new(-1031, 1019), IVec2::new(1_048_575, -1_048_575)] {
            for steep in [false, true] {
                for creek in [Creek(None, None), Creek(Some(centre.x + 9), None), Creek(None, Some(centre.y + 5))] {
                    let land = WalkingLand(creek, steep);
                    for seed in [1, 7, 42] {
                        for wiggle in [0.0, 0.3, 1.5] {
                            for reach in [8, 48] {
                                let (mut a, mut b) = (Ground::new(&land), Ground::new(&land));
                                let (mut ra, mut rb) = (Rng::new(seed, 123, 0, 0), Rng::new(seed, 123, 0, 0));
                                let bounds = Rect { min: centre - reach, max: centre + reach };
                                let from = centre.as_vec2() - Vec2::new(3.0, 2.0);
                                let angle = seed as f32 * 0.1;
                                let lanes = if seed == 7 { vec![Lane::new(vec![centre.as_vec2() - Vec2::new(10.0, 10.0), centre.as_vec2() + Vec2::splat(10.0)], 1.0, false, false)] } else { Vec::new() };
                                let avoid = [(Rect::new(centre + IVec2::new(8, -6), IVec2::new(4, 6)), CLEAR)];
                                let expected = scalar(&mut a, &lanes, &mut ra, from, angle, 40.0, wiggle, &bounds, &rules, &avoid);
                                let actual = walk(&mut b, &lanes, &mut rb, from, angle, 40.0, wiggle, &bounds, &rules, &avoid);
                                assert_eq!(actual, expected, "{centre:?}, seed {seed}, wiggle {wiggle}, reach {reach}, hills {steep}");
                                assert_eq!(rb.next_u64(), ra.next_u64(), "a terrain batch must not change any RNG draw");
                                assert_eq!(b.seen, a.seen, "exactly the originally used terrain is retained");
                                assert_eq!(b.misses + b.prefetched, a.misses, "duplicate turn coordinates still require only one read");
                                long += usize::from(actual.len() > 10);
                                short += usize::from(actual.len() < 5);
                            }
                        }
                    }
                }
            }
        }
        assert!(long > 0 && short > 0, "both complete and rejected lane paths must occur ({long}/{short})");
        // A short row at the integer boundary never computes an unused suffix.
        let land = Creek(None, None);
        let mut ground = Ground::new(&land);
        ground.prefetch(IVec2::new(i32::MAX, 0), 1);
        assert_eq!(ground.seen.len(), 1);
        assert_eq!(ground.at(IVec2::new(i32::MAX, 0)).height, 140);
    }

    #[test]
    fn water_rings_keep_scalar_decisions_and_read_sets() {
        use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
        struct LandRings {
            pattern: u8,
            reads: Mutex<Vec<(i32, i32)>>,
            batches: AtomicUsize,
        }
        impl Land for LandRings {
            fn sample(&self, x: i32, z: i32) -> GeoSample {
                self.reads.lock().unwrap().push((x, z));
                let wet = match self.pattern {
                    0 => false,
                    1 => x.rem_euclid(17) < 3,
                    2 => z.rem_euclid(23) < 3,
                    3 => true,
                    _ => x.rem_euclid(17) < 3 && z.rem_euclid(23) < 3,
                };
                GeoSample { height: if wet { 139 } else { 140 + (x.rem_euclid(9) + z.rem_euclid(11)) / 5 }, water: 140,
                    mountain: 0.0, river: 0.0, top: 8, body: if self.pattern == 4 { Body::Sea } else { Body::Creek },
                    current: None, cavity: None, crag: Default::default(), trail: None, form: Default::default() }
            }
            fn samples_cached(&self, points: &[(i32, i32)], _scratch: &mut crate::water::Samples) -> Vec<GeoSample> {
                assert!(points.len() <= crate::water::SAMPLE_BATCH);
                self.batches.fetch_add(1, Ordering::Relaxed);
                points.iter().map(|&(x, z)| self.sample(x, z)).collect()
            }
        }
        // The scalar algorithms before grouping. Exact float sums, bank order and
        // cardinal short circuits matter as much as the sampled set.
        fn scalar_direction(ground: &mut Ground, centre: IVec2, reach: i32) -> Option<Vec2> {
            for r in (8..=reach).step_by(8) {
                let mut sum = Vec2::ZERO;
                for k in 0..24 {
                    let dir = Vec2::from_angle(k as f32 * std::f32::consts::TAU / 24.0);
                    let g = ground.at(centre + (dir * r as f32).round().as_ivec2());
                    if (g.submerged() || g.river > 0.5) && g.body != Body::Sea { sum += dir; }
                }
                if sum != Vec2::ZERO { return Some(sum.normalize_or(Vec2::X)); }
            }
            None
        }
        fn scalar_banks(ground: &mut Ground, centre: IVec2, reach: i32) -> Vec<(IVec2, Vec2)> {
            let mut found = Vec::new();
            for r in (10..=reach.max(12)).step_by(3) {
                let n = (r as f32 * std::f32::consts::TAU / 4.0) as i32;
                for k in 0..n {
                    let at = centre + (Vec2::from_angle(k as f32 * std::f32::consts::TAU / n as f32) * r as f32).round().as_ivec2();
                    if !standing(&ground.at(at)) { continue; }
                    for (dx, dz) in [(2, 0), (-2, 0), (0, 2), (0, -2)] {
                        let g = ground.at(at + IVec2::new(dx, dz));
                        if g.submerged() && g.body != Body::Sea {
                            found.push((at, Vec2::new(dx as f32, dz as f32)));
                            break;
                        }
                    }
                }
                if found.len() >= 8 { break; }
            }
            found
        }
        let mut directions = 0;
        let mut banks = 0;
        for pattern in 0..=4 {
            for centre in [IVec2::ZERO, IVec2::new(-31, 19), IVec2::new(1007, -1023)] {
                for reach in [0, 8, 24, 64] {
                    let make = || LandRings { pattern, reads: Mutex::new(Vec::new()), batches: AtomicUsize::new(0) };
                    let (scalar, batched) = (make(), make());
                    let (mut a, mut b) = (Ground::new(&scalar), Ground::new(&batched));
                    let expected = scalar_direction(&mut a, centre, reach);
                    assert_eq!(water_near(&mut b, centre, reach), expected);
                    directions += usize::from(expected.is_some());
                    let expected = scalar_banks(&mut a, centre, reach);
                    assert_eq!(banks_near(&mut b, centre, reach), expected);
                    banks += expected.len();
                    let h = |g: &mut Ground, dx, dz| g.at(centre + IVec2::new(dx, dz)).height as f32;
                    let grad = Vec2::new(h(&mut a, 16, 0) - h(&mut a, -16, 0), h(&mut a, 0, 16) - h(&mut a, 0, -16));
                    assert_eq!(slope(&mut b, centre), (grad.length() >= 2.0).then(|| grad.normalize()));
                    let (mut original, mut grouped) = (scalar.reads.into_inner().unwrap(), batched.reads.into_inner().unwrap());
                    original.sort_unstable();
                    grouped.sort_unstable();
                    assert_eq!(grouped, original, "pattern {pattern}, {centre:?}, reach {reach}: no extra terrain reads");
                    assert!(batched.batches.load(Ordering::Relaxed) > 0);
                }
            }
        }
        assert!(directions > 0 && banks > 0, "positive direction and ordered-bank fixtures must occur");
    }

    #[test]
    fn water_search_only_matters_for_eligible_houses() {
        let mut def = SettlementsDef::default();
        def.extend("base", "base", include_str!("../../../../assets/content/base/settlements.ron")).unwrap();
        let mut boat = def.houses.iter().find(|h| h.name == "boathouse").unwrap().clone();
        let matters = |h: &super::super::def::HouseDef, want: &Want, size: &str, style: &str, built| {
            h.water && house_fits(h, want, size, style, built, def.rules.houses.grand)
        };
        for want in [Want::Home, Want::Kind(Kind::Inn), Want::Kind(Kind::Trader), Want::Work("shrine".into())] {
            assert!(!matters(&boat, &want, "village", "timber_frame", 0), "boathouse cannot serve {want:?}");
        }
        assert!(matters(&boat, &Want::Workshop, "village", "timber_frame", 0));
        boat.weight = 0.0;
        assert!(matters(&boat, &Want::Workshop, "village", "timber_frame", 0), "water dependencies conservatively retain zero-weight entries");
        assert!(matters(&boat, &Want::Work("boathouse".into()), "village", "timber_frame", 0));
        assert!(!matters(&boat, &Want::Workshop, "village", "timber_frame", 1), "built-count cap removes its water dependency");
        boat.sizes = vec!["village".into()];
        assert!(!matters(&boat, &Want::Workshop, "hamlet", "timber_frame", 0));
        boat.not_in = vec!["timber_frame".into()];
        assert!(!matters(&boat, &Want::Workshop, "village", "timber_frame", 0));
        boat.not_in.clear();
        // A mod can add a water-only home: this is eligibility, not a boathouse-name
        // exception or an assumption that all future homes are dry.
        boat.work = None;
        assert!(matters(&boat, &Want::Home, "village", "timber_frame", 0));
        assert!(matters(&boat, &Want::Kind(Kind::Trader), "village", "timber_frame", 0));
        boat.width.1 = def.rules.houses.grand;
        assert!(matters(&boat, &Want::Kind(Kind::Inn), "village", "timber_frame", 0));
        boat.water = false;
        assert!(!matters(&boat, &Want::Home, "village", "timber_frame", 0));
    }

    #[test]
    fn lazy_name_exclusions_match_all_neighbour_offers() {
        let mut def = SettlementsDef::default();
        def.extend("base", "base", include_str!("../../../../assets/content/base/settlements.ron")).unwrap();
        def.cultures.extend("base", "base", include_str!("../../../../assets/content/base/cultures.ron")).unwrap();
        // An alias in a different family must participate too, including echoes.
        def.cultures.families[1].names = def.cultures.families[0].names.clone();
        let mut positives = 0;
        let mut negatives = 0;
        for seed in [1, 7, 42, 0x5eed] {
            for cell in [IVec2::new(-17, 8), IVec2::new(5, -11)] {
                let neighbours: Vec<_> = (-2..=2).flat_map(|z| (-2..=2).map(move |x| cell + IVec2::new(x, z))).filter(|&c| c != cell).collect();
                let mut all = rustc_hash::FxHashSet::default();
                for &other in &neighbours {
                    for l in 0..def.cultures.families.len() {
                        for k in 0..NAME_OFFERS {
                            for small in [false, true] {
                                if small && def.cultures.families[l].names.small.is_empty() { continue; }
                                all.insert(offered_english(&def, seed, other, l, k, small));
                            }
                        }
                    }
                }
                let mut offered = vec![None; def.cultures.families.len()];
                let mut check = |name: &str| {
                    let expected = all.contains(name);
                    positives += usize::from(expected);
                    negatives += usize::from(!expected);
                    assert_eq!(name_is_taken(&def, seed, &neighbours, &mut offered, name), expected, "seed {seed}, cell {cell}, name {name}");
                };
                for name in &all { check(name); }
                for names in def.cultures.families.iter().map(|f| &f.names) {
                    for first in names.first.iter().take(3) {
                        for last in &names.last { check(&format!("{}{}", first.0, last.0)); }
                        for small in &names.small { check(&format!("{} {}", first.0, small.0)); }
                    }
                }
                check("No family can offer this name");
                let mut unused = vec![None; def.cultures.families.len()];
                assert!(!name_is_taken(&def, seed, &neighbours, &mut unused, "No family can offer this name"));
                assert!(unused.iter().all(Option::is_none), "unrelated families must not draw offers");
            }
        }
        assert!(positives > 1000 && negatives > 1000, "both taken and available candidates must be compared");
    }

    #[test]
    fn lane_paving_bounds_keep_full_scan_points_and_order() {
        let mut clipped = 0;
        let mut admitted = 0;
        let mut empty = 0;
        for offset in [0, -8192, 1_048_576, -16_777_216, 16_777_216, i32::MIN + 128, i32::MAX - 128] {
            let shift = IVec2::new(offset, offset);
            let raster = Rect { min: shift - 40, max: shift + 40 };
            for points in [
                vec![], vec![Vec2::ZERO], vec![Vec2::new(5.0, 0.0); 2],
                vec![Vec2::new(-30.25, 8.5), Vec2::new(18.75, -4.5), Vec2::new(28.0, 50.0)],
                vec![Vec2::new(5.0, -50.0), Vec2::new(5.0, 50.0)],
                vec![Vec2::new(50.0, 50.0), Vec2::new(70.0, 70.0)],
                vec![Vec2::new(-50.0, -50.0), Vec2::new(50.0, 50.0)],
            ] {
                let points: Vec<_> = points.into_iter().map(|p| p + shift.as_vec2()).collect();
                let mut lane = Lane::bare(points, 1.0, true, true);
                let boundary = lane.distance((shift + IVec2::new(8, 2)).as_vec2());
                for half in [0.0, 0.5, 1.0, 3.5, 50.0, boundary.next_down(), boundary, boundary.next_up(), f32::MAX] {
                    lane.half = half;
                    let bounds = lane.paving_bounds(raster);
                    clipped += usize::from(bounds.is_some_and(|r| r.min != raster.min || r.max != raster.max));
                    empty += usize::from(bounds.is_none());
                    let scan = |rect: Option<Rect>| {
                        let mut out = Vec::new();
                        if let Some(rect) = rect {
                            for z in rect.min.y..=rect.max.y {
                                for x in rect.min.x..=rect.max.x {
                                    let p = IVec2::new(x, z);
                                    if lane.distance(p.as_vec2()) <= half { out.push(p); }
                                }
                            }
                        }
                        out
                    };
                    let original = scan(Some(raster));
                    admitted += original.len();
                    assert_eq!(scan(bounds), original, "offset {offset}, half {half}, points {:?}", lane.points);
                }
            }
        }
        assert!(clipped > 100 && admitted > 1000 && empty > 0, "need clipped, painted and rejected positive controls");
        // The computed projection endpoint differs from b after cancellation.
        let a = Vec2::new(1_048_576.0, 0.0);
        let b = Vec2::new(0.03125, 10.0);
        assert_ne!(a + (b - a), b);
        let lane = Lane::bare(vec![a, b], 0.0, true, true);
        let raster = Rect { min: IVec2::new(-2, 8), max: IVec2::new(2, 12) };
        assert!(lane.paving_bounds(raster).unwrap().holds(IVec2::new(0, 10)));
        for points in [vec![Vec2::splat(f32::INFINITY)], vec![Vec2::splat(-f32::MAX), Vec2::splat(f32::MAX)]] {
            let lane = Lane::bare(points, 1.0, true, true);
            let bounds = lane.paving_bounds(raster).unwrap();
            assert_eq!((bounds.min, bounds.max), (raster.min, raster.max), "non-finite arithmetic keeps the original scan");
        }
        let capped = Lane::bare(vec![Vec2::splat(1e30)], f32::MAX, true, true);
        assert_eq!(capped.distance(Vec2::ZERO), f32::MAX, "the original minimum caps overflowing distances");
        let bounds = capped.paving_bounds(raster).unwrap();
        assert_eq!((bounds.min, bounds.max), (raster.min, raster.max));
        for raster in [
            Rect { min: IVec2::splat(i32::MIN), max: IVec2::splat(i32::MIN + 4) },
            Rect { min: IVec2::splat(i32::MAX - 4), max: IVec2::splat(i32::MAX) },
        ] {
            let lane = Lane::bare(vec![raster.min.as_vec2()], 0.0, true, true);
            let bounds = lane.paving_bounds(raster).unwrap();
            assert_eq!((bounds.min, bounds.max), (raster.min, raster.max), "literal integer endpoints round to the same point");
        }
    }

    #[test]
    fn lane_rect_predicates_keep_boundaries_and_stop_at_a_match() {
        let rect = Rect::new(IVec2::ZERO, IVec2::splat(3));
        let make = |points| Lane::new(points, 1.0, false, false);
        let boundary = make(vec![Vec2::new(5.0, -5.0), Vec2::new(5.0, 5.0)]);
        assert_eq!(boundary.rect_distance(&rect), 2.5);
        assert!(!boundary.rect_any(&rect, |d| d < 2.5));
        assert!(boundary.rect_any(&rect, |d| d <= 2.5));
        for points in [
            Vec::new(), vec![Vec2::ZERO], vec![Vec2::new(5.0, 0.0); 2],
            vec![Vec2::new(-30.0, 8.0), Vec2::new(18.0, -4.0)],
            vec![Vec2::new(-20.0, 20.0), Vec2::new(20.0, 20.0), Vec2::new(1.0, 1.0)],
            boundary.points.clone(),
        ] {
            let lane = make(points);
            let distance = lane.rect_distance(&rect);
            for reach in [0.0, 0.5, 2.5, 5.0, 50.0] {
                assert_eq!(lane.rect_any(&rect, |d| d < reach), distance < reach);
                assert_eq!(lane.rect_any(&rect, |d| d <= reach), distance <= reach);
            }
        }
        // Bounds are built from actual rounded interpolation samples. Exercise equal
        // and adjacent-float reaches after translating to large world coordinates.
        for offset in [0.0, -8192.0, 1_048_576.0, -16_777_216.0] {
            let shift = Vec2::new(offset, -offset);
            for points in [
                vec![Vec2::new(5.0, -5.0), Vec2::new(5.0, 5.0)],
                vec![Vec2::new(-30.25, 8.5), Vec2::new(18.75, -4.5), Vec2::new(28.0, 50.0)],
                vec![Vec2::new(50.0, 50.0); 2],
            ] {
                let lane = make(points.into_iter().map(|p| p + shift).collect());
                let rect = Rect::new(shift.as_ivec2(), IVec2::splat(3));
                let distance = lane.rect_distance(&rect);
                for reach in [distance.next_down(), distance, distance.next_up()] {
                    assert_eq!(lane.rect_any(&rect, |d| d < reach), distance < reach, "strict {offset}/{reach}");
                    assert_eq!(lane.rect_any(&rect, |d| d <= reach), distance <= reach, "inclusive {offset}/{reach}");
                }
                let clone = lane.clone();
                assert!(Arc::ptr_eq(lane.samples.as_ref().unwrap(), clone.samples.as_ref().unwrap()));
            }
        }
        let oversized = make(vec![Vec2::splat(10.0), Vec2::new(20_000.0, 10.0)]);
        assert!(oversized.samples.is_none(), "oversized sequences must use exact on-demand sampling");
        let distance = oversized.rect_distance(&rect);
        for reach in [10.0, 20.0, distance.next_down(), distance, distance.next_up()] {
            assert_eq!(oversized.rect_any(&rect, |d| d < reach), distance < reach);
            assert_eq!(oversized.rect_any(&rect, |d| d <= reach), distance <= reach);
        }
        let near = make(vec![Vec2::ONE, Vec2::splat(1000.0)]);
        profile(true);
        let before = LANE_COUNTS.with(|c| c.get());
        assert!(near.rect_any(&rect, |d| d < 1.0));
        let after = LANE_COUNTS.with(|c| c.get());
        profile(false);
        assert_eq!(after[1] - before[1], 1, "a known intersecting point must stop the check");
        let far = make(vec![Vec2::splat(50.0), Vec2::splat(500.0)]);
        assert!(far.samples.as_ref().unwrap().bounds.len() > 10);
        profile(true);
        let before = LANE_COUNTS.with(|c| c.get());
        assert!(!far.rect_any(&rect, |d| d <= 3.0));
        let after = LANE_COUNTS.with(|c| c.get());
        profile(false);
        assert_eq!(after[1] - before[1], 0, "the whole bound excludes every sampled point");
        assert_eq!(after[3] - before[3], 1, "far lanes need only the whole bound");
    }

    /// Flat ground with a creek one block wide along `x == creek.0` (a column) or
    /// `z == creek.1` (a row).
    struct Creek(Option<i32>, Option<i32>);

    impl Land for Creek {
        fn sample(&self, x: i32, z: i32) -> GeoSample {
            let wet = self.0 == Some(x) || self.1 == Some(z);
            let (height, water) = if wet { (139, 140) } else { (140, 140) };
            GeoSample { height, mountain: 0.0, river: 0.0, water, top: 8, body: Body::Creek, current: None, cavity: None, crag: Default::default(), trail: None, form: Default::default() }
        }
    }

    #[test]
    fn terrain_batches_reuse_site_work() {
        let world = super::super::tests::base_world(7);
        let land = world.land();
        let first = IVec2::new(-1904, 528);
        let (step, size) = (16, 25);
        let samples = land.grid(first, step, size);
        let point = |i: usize| first + IVec2::new(i as i32 % size, i as i32 / size) * step;
        let wet = samples.iter().enumerate().find_map(|(i, g)| crate::water::freshwater(g).then(|| point(i))).expect("actual wet full sample");
        let mut water = crate::water::Freshwater::default();
        land.remember_freshwater_grid(first, step, size, &samples, &mut water);
        // A non-grid wet centre builds real sealing scratch; a proven dry centre
        // keeps only the separate Boolean witness, never an invented GeoSample.
        let close = wet + IVec2::ONE;
        land.freshwater_near(close, 0, 1, &mut water);
        let mut absence = crate::water::Samples::default();
        let dry = (0..samples.len()).map(|i| point(i) + IVec2::ONE)
            .find(|&p| land.freshwater_absent_cached(p, &mut absence)).expect("actual absent-channel witness");
        assert!(!land.freshwater_near(dry, 0, 1, &mut water));
        let work = water.work();
        assert!(work[2] > 0 && work[3] > 0, "real corners and raw columns must transfer");
        let mut ground = Ground::from_site(&land, water);
        assert!(ground.seen.len() >= samples.len() && ground.dry_water.contains(&dry));
        assert_eq!((ground.queries, ground.misses, ground.prefetched, ground.clears), (0, 0, 0, 0));
        let mut original = Ground::new(&land);
        for (i, &sample) in samples.iter().enumerate() {
            let p = point(i);
            assert_eq!(ground.at(p), original.at(p));
            assert_eq!(ground.at(p), sample, "every transferred full GeoSample agrees");
        }
        assert_eq!(ground.misses, 0, "warm site terrain performs no new reads");
        assert_eq!(original.misses, samples.len(), "the original empty cache actually reads every point");
        assert_eq!(ground.scratch.work(), work, "warm hits do not rebuild sampler scratch");
        assert!(!ground.freshwater(dry));
        assert_eq!(ground.misses, 0, "the Boolean absence witness avoids a terrain read");
        assert_eq!(ground.at(dry), land.sample(dry.x, dry.y));
        assert_eq!(ground.misses, 1, "a dry witness cannot answer a full terrain query");
        assert!(!ground.dry_water.contains(&dry));
        ground.dry_water.insert(wet);
        assert!(ground.freshwater(wet), "a wet full sample takes precedence over a poisoned false witness");
        ground.dry_water.remove(&wet);
        // Continuing work retains the unchanged joint cap and recollects even a
        // formerly cached first point after clearing, using full original terrain.
        let keep = first;
        ground.seen.get_mut(&keep).unwrap().height = 9999;
        ground.dry_water.insert(keep);
        let fill = GROUND_SAMPLE_LIMIT - ground.seen.len() - ground.dry_water.len();
        ground.dry_water.extend((0..fill).map(|i| IVec2::new(200_000 + i as i32, 200_000)));
        assert_eq!(ground.seen.len() + ground.dry_water.len(), GROUND_SAMPLE_LIMIT);
        let points: Vec<_> = (0..crate::water::SAMPLE_BATCH).map(|i| keep + IVec2::new(i as i32, 0)).collect();
        ground.prefetch_unique(&points);
        assert_eq!(ground.clears, 1);
        assert!(ground.dry_water.is_empty());
        assert_eq!(ground.seen.len(), points.len());
        for p in points { assert_eq!(ground.at(p), land.sample(p.x, p.y)); }
        let empty = Ground::from_site(&land, crate::water::Freshwater::default());
        assert!(empty.seen.is_empty() && empty.dry_water.is_empty());
    }

    #[test]
    fn house_water_absence_keeps_original_spokes_and_full_samples() {
        let mut wet_count = 0;
        let mut dry_count = 0;
        let mut skipped = 0;
        for (seed, centres) in [(7, [IVec2::new(-1776, 752), IVec2::new(-336, 816)]),
            (42, [IVec2::new(720, -784), IVec2::new(-1776, 752)])]
        {
            let world = super::super::tests::base_world(seed);
            let land = world.land();
            let (mut original, mut fast) = (Ground::new(&land), Ground::new(&land));
            for centre in centres {
                for offset in [Vec2::ZERO, Vec2::new(0.49, -0.51), Vec2::new(-0.5, 0.5)] {
                    for i in 0..25 {
                        let at = (centre + IVec2::new(i % 5 - 2, i / 5 - 2) * 16).as_vec2() + offset;
                        let rounded = at.round().as_ivec2();
                        let expected = (0..8).any(|k| {
                            let g = original.at(rounded + (Vec2::from_angle(k as f32 * std::f32::consts::FRAC_PI_4) * 7.0).round().as_ivec2());
                            (g.submerged() || g.river > 0.5) && g.body != Body::Sea
                        });
                        assert_eq!(wet(&mut fast, at), expected, "seed{seed} at{at}");
                        wet_count += usize::from(expected);
                        dry_count += usize::from(!expected);
                        assert!(fast.seen.len() + fast.dry_water.len() <= GROUND_SAMPLE_LIMIT);
                    }
                }
            }
            assert!(fast.misses <= original.misses);
            skipped += original.misses - fast.misses;
            if let Some((&p, &sample)) = original.seen.iter().find(|(_, g)| crate::water::freshwater(g)) {
                fast.seen.insert(p, sample);
                fast.dry_water.insert(p);
                assert!(fast.freshwater(p), "a genuinely wet full sample takes precedence over a poisoned false witness");
                fast.dry_water.remove(&p);
            }
            assert!(!fast.dry_water.is_empty(), "real absence proof must be used");
            for &p in fast.dry_water.iter().take(16) {
                assert!(!crate::water::freshwater(&land.sample(p.x, p.y)), "every negative witness agrees with complete terrain");
            }
            // Full terrain is authoritative and removes its former negative witness.
            let p = *fast.dry_water.iter().next().unwrap();
            let expected = land.sample(p.x, p.y);
            assert_eq!(fast.at(p), expected);
            assert!(!fast.dry_water.contains(&p));
            assert_eq!(fast.freshwater(p), crate::water::freshwater(&expected));
            // Fill only unused false witnesses. Both scalar and row-batch pressure
            // must discard them, and still return the full original terrain.
            for scalar in [true, false] {
                let fill = GROUND_SAMPLE_LIMIT - fast.seen.len() - fast.dry_water.len();
                fast.dry_water.extend((0..fill).map(|i| IVec2::new(200_000 + i as i32, 200_000)));
                assert_eq!(fast.seen.len() + fast.dry_water.len(), GROUND_SAMPLE_LIMIT);
                let p = IVec2::new(-200_000 - i32::from(scalar), -200_000);
                assert!(!fast.seen.contains_key(&p) && !fast.dry_water.contains(&p));
                let before = fast.clears;
                if scalar { assert_eq!(fast.at(p), land.sample(p.x, p.y)); }
                else {
                    fast.prefetch(p, 7);
                    for i in 0..7 { assert_eq!(fast.at(p + IVec2::new(i, 0)), land.sample(p.x + i, p.y)); }
                }
                assert_eq!(fast.clears, before + 1);
                assert!(fast.dry_water.is_empty());
            }
            let fill = GROUND_SAMPLE_LIMIT - fast.seen.len();
            fast.dry_water.extend((0..fill).map(|i| IVec2::new(200_000 + i as i32, 200_000)));
            assert_eq!(fast.seen.len() + fast.dry_water.len(), GROUND_SAMPLE_LIMIT);
            assert!(!fast.seen.contains_key(&p) && !fast.dry_water.contains(&p));
            let before = (fast.clears, fast.misses);
            assert!(!fast.freshwater(p));
            assert_eq!((fast.clears, fast.misses), (before.0 + 1, before.1));
            assert!(fast.seen.is_empty());
            assert_eq!(fast.dry_water.len(), 1, "absence insertion must reset both stores at their joint cap without reading geology");
        }
        assert!(wet_count > 0 && dry_count > 0 && skipped > 0, "actual wet/dry decisions and saved terrain reads are required");
    }

    #[test]
    fn growing_ground_keeps_samples_without_clearing() {
        let land = Creek(Some(-3), None);
        let mut ground = Ground::new(&land);
        ground.seen.reserve(4096);
        let capacity = ground.seen.capacity();
        assert!(capacity >= 4096 && capacity < GROUND_SAMPLE_LIMIT / 8 * 7);
        let keep = IVec2::new(-3, 0);
        let sample = land.sample(keep.x, keep.y);
        ground.seen.insert(keep, sample);
        ground.seen.extend((1..capacity).map(|i| (IVec2::new(100_000 + i as i32, 100_000), sample)));
        assert_eq!(ground.seen.len(), capacity);
        let retained = ground.seen.clone();
        let first = IVec2::new(-9, -17);
        ground.prefetch(first, 7);
        for i in 0..7 {
            let p = first + IVec2::new(i, 0);
            assert_eq!(ground.at(p), land.sample(p.x, p.y));
        }
        assert_eq!(ground.at(keep), sample);
        assert_eq!(ground.clears, 0);
        assert_eq!(ground.seen.len(), capacity + 7);
        assert!(retained.iter().all(|(p, g)| ground.seen.get(p) == Some(g)));
        assert!(ground.seen.capacity() >= GROUND_SAMPLE_LIMIT / 8 * 7, "the large growth path must actually run");
        let prior_full_capacity = FxHashMap::<IVec2, GeoSample>::with_capacity_and_hasher(GROUND_SAMPLE_LIMIT, Default::default()).capacity();
        assert!(ground.seen.capacity() <= prior_full_capacity, "allocation stays within ordinary growth at the original full limit");
    }

    #[test]
    fn planning_prefetch_counts_lookahead_before_a_water_rejection() {
        struct Counted {
            creek: Creek,
            reads: std::sync::atomic::AtomicUsize,
        }
        impl Land for Counted {
            fn sample(&self, x: i32, z: i32) -> GeoSample {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.creek.sample(x, z)
            }
        }
        let land = Counted { creek: Creek(Some(-3), None), reads: 0.into() };
        let mut ground = Ground::new(&land);
        let rect = Rect::new(IVec2::ZERO, IVec2::new(64, 3));
        assert_eq!(level(&mut ground, &rect, 3, 2), None, "first bank block contains water");
        let reads = land.reads.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(ground.queries, 1, "the original row-major predicate still rejects immediately");
        assert_eq!(ground.misses + ground.prefetched, reads, "counters include every actual terrain read");
        assert_eq!(reads, crate::water::SAMPLE_BATCH, "lookahead is bounded by one batch");
        assert!(ground.at(IVec2::new(-3, -3)).submerged());
        assert_eq!(land.reads.load(std::sync::atomic::Ordering::Relaxed), reads, "prefetched terrain is reused exactly");
        let dry = Counted { creek: Creek(None, None), reads: 0.into() };
        let mut ground = Ground::new(&dry);
        let first = IVec2::new(100, 100);
        let sample = dry.creek.sample(100, 100);
        ground.seen.extend((0..GROUND_SAMPLE_LIMIT as i32 - 2).map(|k| (IVec2::new(100_000 + k, 100_000), sample)));
        ground.seen.insert(first, sample);
        // A short final chunk straddles the cap. Clear before collecting misses so its
        // formerly cached first position is included in the same physical batch.
        ground.prefetch(first, 7);
        assert_eq!(ground.clears, 1);
        assert_eq!(ground.seen.len(), 7);
        assert_eq!(ground.prefetched, 7);
        for x in 0..7 { assert_eq!(ground.at(first + IVec2::new(x, 0)), sample); }
        assert_eq!(ground.misses, 0, "clearing must not omit previously cached positions from the batch");
        assert_eq!(dry.reads.load(std::sync::atomic::Ordering::Relaxed), 7);
    }

    /// A lot sampled every other block still sees a creek one block wide on any row or
    /// column of its banks or under it: an odd one, between the samples, as well as an
    /// even one.
    #[test]
    fn a_creek_one_block_wide_is_never_missed() {
        let rect = Rect::new(IVec2::new(0, 0), IVec2::new(10, 8));
        let bank = 3;
        let dry = Creek(None, None);
        assert_eq!(level(&mut Ground::new(&dry), &rect, bank, 2), Some((140, 140)));
        for at in -bank..=9 + bank {
            for creek in [Creek(Some(at), None), Creek(None, Some(at.min(7 + bank)))] {
                let found = level(&mut Ground::new(&creek), &rect, bank, 2);
                assert_eq!(found, None, "a creek at x {:?} z {:?} beside a lot from 0,0 to 9,7 let it stand", creek.0, creek.1);
            }
        }
    }
}
