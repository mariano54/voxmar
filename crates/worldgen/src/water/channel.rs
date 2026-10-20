//! Drawing one tier's rivers and lakes into the land, a region at a time.
//!
//! A river runs from cell to cell along a Catmull-Rom curve through points jittered off
//! the cell centres, so it winds instead of following the grid. Its surface is set at the
//! cells and falls along each edge: gently, which with whole water blocks gives a
//! staircase of small steps that reads as flowing water and rapids, or, where it drops
//! three blocks or more, as a waterfall onto a plunge pool. Every column then takes the
//! nearest river: inside the channel the bed lies under the surface, deeper in the middle
//! (where wide, calm rivers sometimes part around an island); outside, the shore starts
//! level with the water and climbs its first block gently, then the valley rises to the
//! land, and a low berm keeps the water in where the land between cells dips below it.
//!
//! Lakes are drawn on a grid of 8-block cells: every column in the lake's cells whose
//! ground lies under the lake's surface holds water, so the shore follows the real land.
//! Where the land between cells dips below the surface at the shore, the column-level
//! seal raises it (see `water::seal`), so no water hangs in the air.

use std::sync::Arc;

use fastnoise_lite::FastNoiseLite;
use voxmar_engine::glam::{IVec2, Vec2};

use super::cache::Weigh;
use super::drainage::Lake;
use crate::geology::smoothstep;
use crate::rng::hash3;

/// Pieces of straight line per edge.
const SUB: usize = 4;
/// Size of the cells that index edges and lakes, in blocks.
pub const MAP: i32 = 8;
/// Index cells per map cell side for edges.
const INDEX: i32 = 16;
/// Where a river breaching a rim cuts this far under the ground beyond its tier's usual
/// incision (blocks, from none to full), its valley closes into a slot gorge: a ledge
/// beside the water and walls climbing `GORGE_WALL` blocks out per block up, wavering by
/// under a block every dozen or so.
const GORGE: (f32, f32) = (3.0, 8.0);
const GORGE_WALL: f32 = 0.3;

/// What kind of water lies nearest a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Body {
    /// A creek, up to a few blocks wide.
    Creek,
    River,
    Lake,
    /// A closed basin dried to a salt flat.
    Salt,
    /// A hot spring's warm pool.
    Spring,
    Sea,
}

/// How a tier draws its rivers.
pub struct Style {
    /// Rivers at least this wide are `Body::River`, narrower ones creeks.
    pub river_width: f32,
    /// Valley walls: the floor beyond the bank, as a multiple of the half width, and how
    /// many blocks out the wall reaches per block it climbs.
    pub floodplain: f32,
    pub wall: f32,
    /// How far, in blocks, the shore takes to climb its first block from the water.
    pub shore_run: f32,
    /// Furthest a valley reaches beyond its bank, in blocks.
    pub valley: f32,
    /// How far node points are jittered off cell centres, in blocks.
    pub jitter: f32,
    /// How far lake cells' water may reach past their cell, in map cells.
    pub shore: i32,
    /// Ground this far over a tunnel's water stays as a roof.
    pub roof: f32,
    /// Where islands rise in the middle of wide, calm rivers: above 0.35 or so.
    pub islands: FastNoiseLite,
}

/// One edge of a river, from a cell to the one it drains into.
pub struct Edge {
    points: [Vec2; SUB + 1],
    /// Immutable projection values, included in the channel cache's byte weight.
    #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
    segments: Option<Segments>,
    /// River surface at each end, in eighths of a block.
    from: i32,
    to: i32,
    /// Where along the edge (0..1) the river falls from `from` to `to`; negative where it
    /// slopes evenly instead.
    fall: f32,
    half: f32,
    depth: f32,
    /// Furthest from its line the edge changes the ground.
    reach: f32,
    tunnel: bool,
    body: Body,
    /// Blocks of drop per block of length, for currents and rapids.
    slope: f32,
    /// How much of a slot gorge the river runs in at each end, 0 ..= 1 (see `GORGE`).
    gorge: [f32; 2],
}

impl Edge {
    /// Distance from `p` to the edge's line, where along it (0..1) the nearest point
    /// lies, and the direction of flow there.
    fn nearest(&self, p: Vec2) -> (f32, f32, Vec2) {
        #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
        if let Some(segments) = self.segments.as_ref().filter(|_| p.abs().cmple(Vec2::splat(1e15)).all()) {
            // This bound keeps subtraction, both dot products and division finite,
            // even at the minimum denominator. Other values keep the scalar path.
            // SAFETY: this path requires NEON at compile time; the helper reads
            // only its four-element arrays and performs the same unfused operations.
            let (distances, fractions) = unsafe { nearest_four(segments, p) };
            let mut best = (f32::MAX, 0.0, Vec2::X);
            for k in 0..SUB {
                if distances[k] < best.0 {
                    best = (distances[k], (k as f32 + fractions[k]) / SUB as f32, self.points[k + 1] - self.points[k]);
                }
            }
            return best;
        }
        let mut best = (f32::MAX, 0.0, Vec2::X);
        for k in 0..SUB {
            let (a, b) = (self.points[k], self.points[k + 1]);
            let ab = b - a;
            let s = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = p.distance(a + ab * s);
            if d < best.0 {
                best = (d, (k as f32 + s) / SUB as f32, ab);
            }
        }
        best
    }

    /// The river surface at `t` along the edge, in eighths.
    fn surface(&self, t: f32) -> i32 {
        if self.fall >= 0.0 {
            if t < self.fall { self.from } else { self.to }
        } else {
            self.from + ((self.to - self.from) as f32 * t.clamp(0.0, 1.0)).round() as i32
        }
    }

    fn length(&self) -> f32 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// How much of a slot gorge the river runs in at `t` along the edge, 0 ..= 1.
    fn gorge(&self, t: f32) -> f32 {
        self.gorge[0] + (self.gorge[1] - self.gorge[0]) * t.clamp(0.0, 1.0)
    }
}

/// The four immutable segments, one per lane. Preparing them at edge creation
/// avoids transposing the same curve and recomputing its denominators per column.
#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
struct Segments {
    ax: [f32; SUB],
    az: [f32; SUB],
    dx: [f32; SUB],
    dz: [f32; SUB],
    length: [f32; SUB],
}

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
impl Segments {
    fn new(points: &[Vec2; SUB + 1]) -> Option<Self> {
        if !points.iter().all(|p| p.abs().cmple(Vec2::splat(1e15)).all()) { return None; }
        let ab: [Vec2; SUB] = std::array::from_fn(|k| points[k + 1] - points[k]);
        Some(Self {
            ax: std::array::from_fn(|k| points[k].x),
            az: std::array::from_fn(|k| points[k].y),
            dx: ab.map(|v| v.x),
            dz: ab.map(|v| v.y),
            length: ab.map(|v| v.length_squared().max(1e-6)),
        })
    }
}

/// Projection and square root retain their original per-segment arithmetic; the
/// caller still selects in segment order.
#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
unsafe fn nearest_four(segments: &Segments, p: Vec2) -> ([f32; SUB], [f32; SUB]) {
    use std::arch::aarch64::*;
    // SAFETY: all loads and stores address four initialized floats. Every vector
    // operation uses NEON, enabled by this helper's target-feature condition.
    unsafe {
        let (ax, az) = (vld1q_f32(segments.ax.as_ptr()), vld1q_f32(segments.az.as_ptr()));
        let (dx, dz) = (vld1q_f32(segments.dx.as_ptr()), vld1q_f32(segments.dz.as_ptr()));
        let length = vld1q_f32(segments.length.as_ptr());
        let (px, pz) = (vdupq_n_f32(p.x), vdupq_n_f32(p.y));
        let dot = vaddq_f32(vmulq_f32(vsubq_f32(px, ax), dx), vmulq_f32(vsubq_f32(pz, az), dz));
        let mut s = vdivq_f32(dot, length);
        let (zero, one) = (vdupq_n_f32(0.0), vdupq_n_f32(1.0));
        // Strict comparisons also preserve the scalar clamp's signed zeros.
        s = vbslq_f32(vcltq_f32(s, zero), zero, s);
        s = vbslq_f32(vcgtq_f32(s, one), one, s);
        let ex = vsubq_f32(px, vaddq_f32(ax, vmulq_f32(dx, s)));
        let ez = vsubq_f32(pz, vaddq_f32(az, vmulq_f32(dz, s)));
        let d = vsqrtq_f32(vaddq_f32(vmulq_f32(ex, ex), vmulq_f32(ez, ez)));
        let (mut distances, mut fractions) = ([0.0; SUB], [0.0; SUB]);
        vst1q_f32(distances.as_mut_ptr(), d);
        vst1q_f32(fractions.as_mut_ptr(), s);
        (distances, fractions)
    }
}

/// A lake's water or shore over one map cell.
#[derive(Clone, Copy, Default)]
struct LakeCell {
    /// Surface of the lake whose water may fill the cell, in eighths (0: none).
    water: i32,
    /// Surface of the lake whose shore the cell is (0: none).
    shore: i32,
    salt: bool,
    /// Whether the lake is a hot spring.
    spring: bool,
}

/// The rivers one tier draws from one region's cells, and the lakes over the region.
pub struct Channels {
    /// The region whose rivers these are, and whose lakes.
    region: IVec2,
    /// First block of the edge index, which covers the region plus a margin.
    index_origin: IVec2,
    index_size: i32,
    edges: Vec<Edge>,
    /// Edge numbers per index cell: `starts[i]..starts[i + 1]` into `items`.
    starts: Vec<u32>,
    items: Vec<u32>,
    /// Lake cells over the region, one map cell larger on every side.
    lake_origin: IVec2,
    lake_size: i32,
    lakes: Vec<LakeCell>,
}

impl Channels {
    /// A conservative possibility test before sampling land for a freshwater-only
    /// query. With no indexed edge or home lake/shore/salt cell, carving cannot set
    /// a body or wetness, regardless of the original terrain's height.
    pub(super) fn may_have_water(&self, x: i32, z: i32) -> bool {
        if self.near(x, z).next().is_some() { return true; }
        if IVec2::new(x, z).div_euclid(IVec2::splat(super::REGION)) != self.region { return false; }
        let lake = self.lake(x, z);
        lake.water > 0 || lake.shore > 0 || lake.salt
    }

    /// Places along the region's rivers that run deep in a slot gorge: a point near the
    /// deeper end of each such edge, the height of its top water block, and how much of a
    /// gorge it is there, 0 ..= 1.
    pub fn gorges(&self) -> impl Iterator<Item = (Vec2, i32, f32)> + '_ {
        self.edges.iter().filter(|e| !e.tunnel).map(|e| {
            let t = if e.gorge[1] > e.gorge[0] { 0.75 } else { 0.25 };
            (e.points[(t * SUB as f32) as usize], top_block(e.surface(t)), e.gorge(t))
        })
    }
}

impl Weigh for Channels {
    fn bytes(&self) -> usize {
        self.edges.capacity() * std::mem::size_of::<Edge>() + (self.starts.capacity() + self.items.capacity()) * 4 + self.lakes.capacity() * std::mem::size_of::<LakeCell>()
    }
}

/// One edge to draw: everything about a river cell that matters for its curve.
pub struct Span {
    pub from: IVec2,
    pub to: IVec2,
    /// The cell upstream of `from` along the main river, and the one `to` drains into.
    pub before: Option<IVec2>,
    pub after: Option<IVec2>,
    pub levels: (i32, i32),
    /// How far the ground stands over the river at each end, in blocks: deep where the
    /// river breaches a rim.
    pub cut: (f32, f32),
    pub width: f32,
    pub tunnel: bool,
}

impl Channels {
    /// Draws the spans owned by the region at `region` (blocks `REGION` wide) and the lakes
    /// over it, for a tier with cells `cell` blocks wide.
    #[allow(clippy::too_many_arguments)]
    pub fn new(style: &Style, cell: i32, seed: u64, region: IVec2, size: i32, margin: i32, spans: &[Span], lakes: &[Arc<Lake>]) -> Self {
        let place = |c: IVec2| {
            let h = hash3(seed ^ 0x7e1, c.x, 0, c.y);
            let unit = |bits: u64| (bits & 0xffff) as f32 / 65535.0 * 2.0 - 1.0;
            (c * cell + cell / 2).as_vec2() + Vec2::new(unit(h), unit(h >> 16)) * style.jitter
        };
        let edges: Vec<Edge> = spans
            .iter()
            .map(|span| {
                let (p1, p2) = (place(span.from), place(span.to));
                let p0 = span.before.map_or(2.0 * p1 - p2, place);
                let p3 = span.after.map_or(2.0 * p2 - p1, place);
                let mut points = [Vec2::ZERO; SUB + 1];
                for (k, point) in points.iter_mut().enumerate() {
                    let t = k as f32 / SUB as f32;
                    let (t2, t3) = (t * t, t * t * t);
                    *point = 0.5
                        * (2.0 * p1
                            + (p2 - p0) * t
                            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                            + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3);
                }
                let (from, to) = (span.levels.0, span.levels.1.min(span.levels.0));
                let half = span.width / 2.0;
                let depth = (0.24 * span.width).max(1.0);
                let drop = (from - to) as f32 / 8.0;
                let fall = if drop >= 3.0 {
                    let h = hash3(seed ^ 0xfa11, span.from.x, 2, span.from.y);
                    0.25 + 0.3 * (h & 0xffff) as f32 / 65535.0
                } else {
                    -1.0
                };
                let mut edge = Edge {
                    points,
                    #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
                    segments: Segments::new(&points),
                    from,
                    to,
                    fall,
                    half,
                    depth,
                    reach: half + style.valley,
                    tunnel: span.tunnel,
                    body: if span.width >= style.river_width { Body::River } else { Body::Creek },
                    slope: 0.0,
                    gorge: [span.cut.0, span.cut.1].map(|cut| smoothstep(GORGE.0, GORGE.1, cut)),
                };
                edge.slope = drop / edge.length().max(1.0);
                edge
            })
            .collect();

        // Index edges by the cells their reach touches.
        let index_origin = region * size - margin;
        let index_size = (size + 2 * margin) / INDEX;
        let cells = (index_size * index_size) as usize;
        let mut lists: Vec<Vec<u32>> = vec![Vec::new(); cells];
        for (e, edge) in edges.iter().enumerate() {
            let (lo, hi) = edge.points.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), &p| (lo.min(p), hi.max(p)));
            let lo = ((lo - edge.reach).floor().as_ivec2() - index_origin).div_euclid(IVec2::splat(INDEX)).max(IVec2::ZERO);
            let hi = ((hi + edge.reach).ceil().as_ivec2() - index_origin).div_euclid(IVec2::splat(INDEX)).min(IVec2::splat(index_size - 1));
            for j in lo.y..=hi.y {
                for i in lo.x..=hi.x {
                    lists[(j * index_size + i) as usize].push(e as u32);
                }
            }
        }
        let mut starts = Vec::with_capacity(cells + 1);
        let mut items = Vec::new();
        for list in lists {
            starts.push(items.len() as u32);
            items.extend(list);
        }
        starts.push(items.len() as u32);

        // Lakes, rasterised onto map cells over the region and a ring around it.
        let lake_size = size / MAP + 2;
        let lake_origin = region * size / MAP - 1;
        let mut map = vec![LakeCell::default(); (lake_size * lake_size) as usize];
        let inside = |m: IVec2| {
            let p = m - lake_origin;
            ((0..lake_size).contains(&p.x) && (0..lake_size).contains(&p.y)).then(|| (p.y * lake_size + p.x) as usize)
        };
        // Each lake's footprint covers the map cells under its own cells, grown by the
        // shore distance; its shore is one map cell beyond. Map cells further than that
        // from the region cannot reach it, so the work stays within an area around it.
        let per = cell / MAP;
        let grow_by = style.shore + 1;
        let ext_origin = lake_origin - grow_by;
        let ext = lake_size + 2 * grow_by;
        let ext_index = |m: IVec2| {
            let p = m - ext_origin;
            ((0..ext).contains(&p.x) && (0..ext).contains(&p.y)).then(|| (p.y * ext + p.x) as usize)
        };
        let dilate = |cells: &[bool]| -> Vec<bool> {
            (0..ext * ext)
                .map(|i| {
                    let (x, z) = (i % ext, i / ext);
                    (z - 1..=z + 1).any(|j| (x - 1..=x + 1).any(|k| (0..ext).contains(&j) && (0..ext).contains(&k) && cells[(j * ext + k) as usize]))
                })
                .collect()
        };
        for lake in lakes {
            let (wet, salt) = (lake.wet(), lake.salt());
            if !wet && !salt {
                continue;
            }
            let mut footprint = vec![false; (ext * ext) as usize];
            let mut any = false;
            for &c in &lake.cells {
                for j in 0..per {
                    for i in 0..per {
                        let m = c * per + IVec2::new(i, j);
                        if salt && let Some(k) = inside(m) {
                            map[k].salt = true;
                        }
                        if let Some(k) = ext_index(m) {
                            footprint[k] = true;
                            any = true;
                        }
                    }
                }
            }
            if !wet || !any {
                continue;
            }
            for _ in 0..style.shore {
                footprint = dilate(&footprint);
            }
            let ring = dilate(&footprint);
            for j in 0..lake_size {
                for i in 0..lake_size {
                    let m = lake_origin + IVec2::new(i, j);
                    let e = ext_index(m).unwrap();
                    let k = (j * lake_size + i) as usize;
                    if footprint[e] {
                        map[k].water = map[k].water.max(lake.level);
                        map[k].spring |= lake.spring;
                    } else if ring[e] {
                        map[k].shore = map[k].shore.max(lake.level);
                        map[k].spring |= lake.spring;
                    }
                }
            }
        }
        Self { region, index_origin, index_size, edges, starts, items, lake_origin, lake_size, lakes: map }
    }

    /// The edges whose reach may cover a column, if the column lies in this index.
    fn near(&self, x: i32, z: i32) -> impl Iterator<Item = &Edge> {
        let p = IVec2::new(x, z) - self.index_origin;
        let cell = p.div_euclid(IVec2::splat(INDEX));
        let range = if (0..self.index_size).contains(&cell.x) && (0..self.index_size).contains(&cell.y) {
            let i = (cell.y * self.index_size + cell.x) as usize;
            self.starts[i] as usize..self.starts[i + 1] as usize
        } else {
            0..0
        };
        self.items[range].iter().map(|&e| &self.edges[e as usize])
    }

    fn lake(&self, x: i32, z: i32) -> LakeCell {
        let p = IVec2::new(x, z).div_euclid(IVec2::splat(MAP)) - self.lake_origin;
        if (0..self.lake_size).contains(&p.x) && (0..self.lake_size).contains(&p.y) {
            self.lakes[(p.y * self.lake_size + p.x) as usize]
        } else {
            LakeCell::default()
        }
    }
}

/// A river's current at a column: the direction it flows and how fast.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Current {
    /// Direction of flow, in 16ths of a turn counter-clockwise from +x (towards +z).
    pub direction: u8,
    /// 0 (still) ..= 3 (rapids).
    pub speed: u8,
}

/// A river running under the ground: the cave it flows through, as heights of its
/// floor (the top solid block) and its ceiling (the top air block).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cavity {
    pub floor: i32,
    pub ceiling: i32,
}

/// One column after a tier has carved it.
#[derive(Clone, Copy, Debug)]
pub struct Carved {
    pub ground: f32,
    /// River or lake surface over the column, in eighths, where water covers its ground
    /// (judged by `ground` once rounded down).
    pub water: Option<i32>,
    /// Surface of the nearest river or lake that shapes the column, in eighths.
    pub near: Option<i32>,
    pub body: Option<Body>,
    /// 1 in water, falling to 0 at the edge of a valley or shore.
    pub wet: f32,
    pub current: Option<Current>,
    pub cavity: Option<Cavity>,
    /// Whether a river's channel (not a lake) covers the column.
    channel: bool,
}

impl Carved {
    pub fn land(ground: f32) -> Self {
        Self { ground, water: None, near: None, body: None, wet: 0.0, current: None, cavity: None, channel: false }
    }
}

/// Top water block under a surface in eighths.
pub fn top_block(surface: i32) -> i32 {
    (surface - 1).div_euclid(8)
}

/// Carves one tier's rivers and lakes into a column whose ground is `before.ground`:
/// the edges of every region in `regions` whose index covers the column, and the lakes
/// of the region the column lies in, which must be one of them.
pub fn carve(style: &Style, regions: &[Option<Arc<Channels>>], x: i32, z: i32, before: Carved) -> Carved {
    let regions = regions.iter().map(|c| c.as_deref().expect("filled"));
    let home = IVec2::new(x, z).div_euclid(IVec2::splat(super::REGION));
    let land = before.ground;
    let p = Vec2::new(x as f32 + 0.5, z as f32 + 0.5);
    let mut ground = land;
    // The channel the column lies in: (distance over half width, edge, along, flow).
    let mut channel: Option<(f32, &Edge, f32, Vec2)> = None;
    let mut berm = f32::MIN;
    let mut near: Option<(f32, i32, Body)> = None;
    let mut wet = 0.0f32;
    let mut cavity = None;
    let mut own = None;
    for edge in regions.flat_map(|c| {
        if c.region == home {
            own = Some(c);
        }
        c.near(x, z)
    }) {
        let (d, t, along) = edge.nearest(p);
        if d >= edge.reach {
            continue;
        }
        let surface = edge.surface(t);
        let top = top_block(surface) as f32;
        let excess = land - top - 1.0;
        if edge.tunnel && excess > style.roof {
            // Under a ridge the river runs in a tunnel and leaves the ground above alone.
            if d < edge.half + 2.0 {
                let span = edge.half + 2.0;
                let floor = if d < edge.half { top - (edge.depth * (1.0 - (d / edge.half).powi(2))).max(1.0).round() } else { top };
                let ceiling = top + (3.0 + edge.half * 0.5) * (1.0 - (d / span).powi(2)).sqrt();
                if ceiling > floor + 1.0 && cavity.is_none() {
                    cavity = Some((Cavity { floor: floor as i32, ceiling: ceiling as i32 }, surface, d < edge.half, along, edge));
                }
            }
            continue;
        }
        if near.is_none_or(|(nd, _, _)| d < nd) {
            near = Some((d, surface, edge.body));
        }
        if d < edge.half {
            let n = d / edge.half;
            if channel.is_none_or(|(c, ..)| n < c) {
                channel = Some((n, edge, t, along));
            }
            wet = 1.0;
            continue;
        }
        // The valley: a shore level with the water at its edge, climbing its first block
        // gently across a floodplain, then a wall up to the land. Where the river breaches
        // high ground, a slot gorge instead: a narrow ledge, then steep, ragged walls.
        let rise = land - top;
        if rise > 0.0 {
            let gorge = edge.gorge(t);
            let lerp = |a: f32, b: f32| a + (b - a) * gorge;
            let shore = lerp(edge.half * (1.0 + style.floodplain) + style.shore_run, edge.half + 1.5);
            let valley = (shore + (rise - 1.0).max(0.0) * lerp(style.wall, GORGE_WALL)).min(edge.reach);
            let d = if gorge > 0.0 { d + gorge * 0.7 * style.islands.get_noise_2d(x as f32 * 2.5 + 311.0, z as f32 * 2.5) } else { d };
            let profile = if d < shore {
                smoothstep(edge.half, shore, d)
            } else {
                let s = ((d - shore) / (valley - shore).max(1e-3)).clamp(0.0, 1.0);
                1.0 + (rise - 1.0).max(0.0) * lerp(smoothstep(0.0, 1.0, s), s)
            };
            if d < valley.max(shore) {
                ground = ground.min(top + profile.min(rise));
            }
        }
        // The bank holds the water a few blocks upstream too, which around a waterfall
        // leaves a ridge either side of the drop.
        // At sea level the sea itself fills any dip, so no bank is needed there.
        if d < edge.half + 2.0 {
            let upstream = top_block(edge.surface(t - 3.0 / edge.length().max(1.0)));
            if upstream > crate::SEA_LEVEL {
                berm = berm.max(upstream as f32);
            }
        }
        let shore = edge.half + 3.0 + edge.half.min(6.0);
        wet = wet.max(1.0 - smoothstep(edge.half, shore.max(edge.half + 4.0), d));
    }

    let mut out = Carved { ground, wet: wet.max(before.wet), ..before };
    out.near = near.map(|(_, s, _)| s).or(before.near);
    out.body = near.map(|(_, _, b)| b).or(before.body);
    if let Some((n, edge, t, along)) = channel {
        let surface = edge.surface(t);
        let top = top_block(surface) as f32;
        let mut depth = (edge.depth * (1.0 - n * n)).max(1.0);
        // A plunge pool below a waterfall.
        if edge.fall >= 0.0 && t >= edge.fall && t < edge.fall + 0.35 {
            depth = depth.max((edge.depth / 2.0).clamp(3.0, edge.depth.max(3.0)));
        }
        out.channel = true;
        out.body = Some(edge.body);
        // Islands of gravel and grass split wide, calm rivers, never near the banks.
        let island = edge.half >= 4.0 && edge.fall < 0.0 && edge.slope < 0.05 && {
            let v = style.islands.get_noise_2d(x as f32, z as f32);
            v > 0.35 + 0.8 * n * n
        };
        if island {
            out.ground = top + 1.0;
            out.water = before.water;
        } else {
            out.ground = out.ground.min(top - depth.round());
            out.water = Some(out.water.map_or(surface, |w| w.max(surface)));
            out.current = Some(current(edge, along, n, t));
        }
    } else if !before.channel && before.water.is_none() {
        out.ground = out.ground.max(berm);
    }
    if let Some((cave, surface, flooded, along, edge)) = cavity
        && out.water.is_none()
    {
        out.cavity = Some(cave);
        if flooded {
            out.water = Some(surface);
            out.current = Some(current(edge, along, 0.5, 0.5));
        }
        out.near = Some(surface);
        out.body = Some(edge.body);
    }

    // Lakes.
    let lake = own.expect("the column's own region").lake(x, z);
    let lake_body = if lake.spring { Body::Spring } else { Body::Lake };
    if lake.water > 0 && out.cavity.is_none() {
        let top = top_block(lake.water);
        if (out.ground.floor() as i32) < top {
            out.water = Some(out.water.map_or(lake.water, |w| w.max(lake.water)));
            out.body = Some(lake_body);
            if !out.channel {
                out.current = None;
            }
        }
        out.near = Some(out.near.map_or(lake.water, |n| n.max(lake.water)));
        if out.body.is_none() || !out.channel {
            out.body = Some(lake_body);
        }
        out.wet = out.wet.max(1.0 - smoothstep(0.0, 5.0, out.ground - top as f32));
    } else if lake.shore > 0 {
        // The shore: banks of the lake's kind. Where the ground dips below the water
        // beside it, sealing raises it (see `water::seal`).
        let top = top_block(lake.shore) as f32;
        out.near = Some(out.near.map_or(lake.shore, |n| n.max(lake.shore)));
        out.body = out.body.or(Some(lake_body));
        out.wet = out.wet.max(1.0 - smoothstep(0.0, 5.0, out.ground - top));
    }
    if lake.salt && out.water.is_none() {
        out.body = Some(Body::Salt);
        out.wet = out.wet.max(0.4);
    }
    // Water must cover the ground to count.
    if let Some(w) = out.water
        && (out.ground.floor() as i32) >= top_block(w)
        && out.cavity.is_none()
    {
        out.water = None;
        out.current = None;
    }
    out
}

/// The current at a point of an edge, `n` of its half width from the middle and `t`
/// along it.
fn current(edge: &Edge, along: Vec2, n: f32, t: f32) -> Current {
    let angle = along.y.atan2(along.x).rem_euclid(std::f32::consts::TAU);
    let direction = ((angle / std::f32::consts::TAU * 16.0).round() as u8) % 16;
    let falling = edge.fall >= 0.0 && (t - edge.fall).abs() < 0.08;
    let speed: u8 = if falling || edge.slope > 0.3 {
        3
    } else if edge.slope > 0.08 {
        2
    } else {
        1
    };
    // Slack water by the banks.
    let speed = if n > 0.75 { speed.saturating_sub(1).max(1) } else { speed };
    Current { direction, speed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_segments_keep_original_bits_and_ties() {
        fn original(edge: &Edge, p: Vec2) -> (f32, f32, Vec2) {
            let mut best = (f32::MAX, 0.0, Vec2::X);
            for k in 0..4 {
                let (a, b) = (edge.points[k], edge.points[k + 1]);
                let ab = b - a;
                let s = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let d = p.distance(a + ab * s);
                if d < best.0 { best = (d, (k as f32 + s) / 4.0, ab); }
            }
            best
        }
        let bits = |(d, t, v): (f32, f32, Vec2)| [d.to_bits(), t.to_bits(), v.x.to_bits(), v.y.to_bits()];
        let check = |edge: &Edge, p: Vec2| {
            #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
            {
                let bounded = edge.points.iter().all(|q| q.x.abs() <= 1e15 && q.y.abs() <= 1e15);
                assert_eq!(edge.segments.is_some(), bounded, "real bounded curves must use prepared segments");
                if let Some(segments) = &edge.segments {
                    for k in 0..4 {
                        let a = edge.points[k];
                        let ab = edge.points[k + 1] - a;
                        let actual = [segments.ax[k], segments.az[k], segments.dx[k], segments.dz[k], segments.length[k]];
                        let expected = [a.x, a.y, ab.x, ab.y, ab.length_squared().max(1e-6)];
                        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
                    }
                }
            }
            assert_eq!(bits(edge.nearest(p)), bits(original(edge, p)), "{:?} at {p:?}", edge.points);
        };
        let mut edge = Edge { points: [Vec2::ZERO; 5],
            #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
            segments: Segments::new(&[Vec2::ZERO; 5]),
            from: 0, to: 0, fall: -1.0, half: 1.0,
            depth: 1.0, reach: 2.0, tunnel: false, body: Body::Creek, slope: 0.0, gorge: [0.0; 2] };
        let set_points = |edge: &mut Edge, points| {
            edge.points = points;
            #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
            { edge.segments = Segments::new(&points); }
        };
        let curves = [
            [Vec2::ZERO; 5],
            [Vec2::new(-0.0, 0.0), Vec2::new(0.0, -0.0), Vec2::ZERO, Vec2::ONE, Vec2::ZERO],
            [Vec2::new(-2.0, -2.0), Vec2::new(2.0, -2.0), Vec2::new(2.0, 2.0), Vec2::new(-2.0, 2.0), Vec2::new(-2.0, -2.0)],
            [Vec2::new(-7.0, 3.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0), Vec2::new(3.0, -5.0), Vec2::new(-7.0, 3.0)],
            [Vec2::splat(1e-20), Vec2::splat(-1e-20), Vec2::ZERO, Vec2::splat(1e-4), Vec2::splat(-1e-4)],
            [Vec2::splat(1e15), Vec2::splat(-1e15), Vec2::ZERO, Vec2::splat(1e15), Vec2::splat(-1e15)],
            [Vec2::splat(1e30), Vec2::splat(-1e30), Vec2::ZERO, Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)],
            [Vec2::new(f32::NAN, 0.0), Vec2::ZERO, Vec2::ONE, Vec2::new(0.0, f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)],
        ];
        for curve in curves {
            for shift in [Vec2::ZERO, Vec2::new(-1031.0, 1019.0), Vec2::splat(16_777_216.0)] {
                set_points(&mut edge, curve.map(|p| p + shift));
                for z in -8..=8 {
                    for x in -8..=8 {
                        let p = shift + Vec2::new(x as f32 * 0.5, z as f32 * 0.5);
                        check(&edge, p);
                    }
                }
                for q in edge.points {
                    for p in [q, q + Vec2::splat(0.5), q - Vec2::splat(0.5), Vec2::splat(1e15), Vec2::splat(1e30), Vec2::new(f32::NAN, 0.0), Vec2::new(0.0, f32::NAN), Vec2::splat(f32::INFINITY)] {
                        check(&edge, p);
                    }
                }
            }
        }
        // At the centre of this square, all distances tie. The first segment wins.
        set_points(&mut edge, curves[2]);
        assert_eq!(edge.nearest(Vec2::ZERO), (2.0, 0.125, Vec2::new(4.0, 0.0)));

        let registry = Arc::new(crate::test_registry());
        let mut actual_edges = 0;
        for (seed, at) in [(7, IVec2::new(-1776, 752)), (14, IVec2::new(656, -304))] {
            let world = crate::Overworld::new(seed, registry.clone());
            let (trunks, streams) = world.rivers.tiers(world.stages(), at - 1, at + 1);
            for channels in trunks.list().iter().chain(streams.list()).flatten() {
                for edge in &channels.edges {
                    actual_edges += 1;
                    for k in 0..SUB {
                        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                            let p = edge.points[k] + (edge.points[k + 1] - edge.points[k]) * t;
                            for offset in [Vec2::ZERO, Vec2::splat(0.5), Vec2::new(-7.0, 7.0)] { check(edge, p + offset); }
                        }
                    }
                }
            }
        }
        assert!(actual_edges > 0, "the oracle must include real generated river curves");
    }
}
