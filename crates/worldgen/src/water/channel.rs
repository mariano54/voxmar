//! Drawing one tier's rivers and lakes into the land, a region at a time.
//!
//! A river runs from cell to cell along a Catmull-Rom curve through points jittered off
//! the cell centres, so it winds instead of following the grid. Its surface is set at the
//! cells and falls along each edge: gently, which with whole water blocks gives a
//! staircase of small steps that reads as flowing water and rapids, or, where it drops
//! three blocks or more, as a waterfall onto a plunge pool. Every column then takes the
//! nearest river: inside the channel the bed lies under the surface, deeper in the middle
//! (where wide, calm rivers sometimes part around an island); outside, the valley rises
//! from the bank to the land, and a low berm keeps the water in where the land between
//! cells dips below it.
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

/// What kind of water lies nearest a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Body {
    /// A creek, up to a few blocks wide.
    Creek,
    River,
    Lake,
    /// A closed basin dried to a salt flat.
    Salt,
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
}

impl Edge {
    /// Distance from `p` to the edge's line, where along it (0..1) the nearest point
    /// lies, and the direction of flow there.
    fn nearest(&self, p: Vec2) -> (f32, f32, Vec2) {
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
}

/// A lake's water or shore over one map cell.
#[derive(Clone, Copy, Default)]
struct LakeCell {
    /// Surface of the lake whose water may fill the cell, in eighths (0: none).
    water: i32,
    /// Surface of the lake whose shore the cell is (0: none).
    shore: i32,
    salt: bool,
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
                    from,
                    to,
                    fall,
                    half,
                    depth,
                    reach: half + style.valley,
                    tunnel: span.tunnel,
                    body: if span.width >= style.river_width { Body::River } else { Body::Creek },
                    slope: 0.0,
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
                    } else if ring[e] {
                        map[k].shore = map[k].shore.max(lake.level);
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
        // The valley: a floor beyond the bank, then a wall up to the land.
        if excess > 0.0 {
            let valley = (edge.half * (1.0 + style.floodplain) + 1.5 + excess * style.wall).min(edge.reach);
            if d < valley {
                ground = ground.min(top + 1.0 + excess * smoothstep(edge.half * (1.0 + style.floodplain), valley, d));
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
    if lake.water > 0 && out.cavity.is_none() {
        let top = top_block(lake.water);
        if (out.ground.floor() as i32) < top {
            out.water = Some(out.water.map_or(lake.water, |w| w.max(lake.water)));
            out.body = Some(Body::Lake);
            if !out.channel {
                out.current = None;
            }
        }
        out.near = Some(out.near.map_or(lake.water, |n| n.max(lake.water)));
        if out.body.is_none() || !out.channel {
            out.body = Some(Body::Lake);
        }
        out.wet = out.wet.max(1.0 - smoothstep(0.0, 5.0, out.ground - top as f32));
    } else if lake.shore > 0 {
        // The shore: banks of the lake's kind. Where the ground dips below the water
        // beside it, sealing raises it (see `water::seal`).
        let top = top_block(lake.shore) as f32;
        out.near = Some(out.near.map_or(lake.shore, |n| n.max(lake.shore)));
        out.body = out.body.or(Some(Body::Lake));
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
