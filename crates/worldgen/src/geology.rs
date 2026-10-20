//! Stage 1: the land. Continents, mountain ranges, valleys, plateaus and the lowland's
//! hills and terraces make the relief the climate sees (stage 2, `climate`); the terrain
//! shapes that rise with the climate (`shapes`) stand on it, and rivers and lakes cut into
//! the result (`water`).
//!
//! The relief is read from the land's router (`crate::router`, data in `land.ron`): a few
//! large noise fields define a point at every position (continentalness, erosion, ridges,
//! a region selector, and the base climate), and nested splines turn it into the height of
//! the land, as in Minecraft's noise router and Tectonic, whose splines `land.ron` adapts.
//! Coasts follow the folded continental noise, so the sea runs in a connected web of
//! channels around large landmasses, with shores fractal down to a few dozen blocks;
//! ranges follow the zero lines of the erosion noise, so they run in long chains; valleys
//! follow the ridge noise's zero lines, a network trunk rivers find; and the lowlands
//! between take the style their region and climate call for.
//!
//! Heights above the sea are then scaled with the world's mountains, stretched by their
//! square, so lowlands stay low while ranges rise high, and eased under a ceiling well
//! below the top of the world, so nothing is cut flat.
//!
//! The land reads the climate's base lattice (sea-level temperature and precipitation,
//! which read only continents and ranges), and the regional climate reads the land; shapes
//! read the regional climate, so the climate never sees them: a karst tower does not cool
//! its own summit. Both climate lattices are cached here in tiles, shared by the shapes and
//! by every sampler of the climate, so all see exactly the same fields.

use std::cell::RefCell;
use std::sync::Arc;

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use rustc_hash::FxHashMap;
use voxmar_engine::glam::IVec2;

use crate::climate::{self, Base, Climate, Regional};
use crate::router::{LANES, Lanes, NoClimate, Program, Router};
use crate::content::WorldContent;
use crate::dials::WorldDials;
use crate::rocks::Rocks;
use crate::shapes::{Ground, ShapeId, Shapes};
use crate::water::cache::Cache;
use crate::density::{Crag, Density};
use crate::water::{Body, Cavity, Current};
use crate::{Overworld, SEA_LEVEL};

/// The ground at one position, with its rivers and lakes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoSample {
    /// Height of the top ground block, rivers and lakes carved in.
    pub height: i32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
    /// 1 in a river channel or lake, falling to 0 at the edge of its valley or shore.
    pub river: f32,
    /// Height of the top water block over the column where it is above `height`;
    /// otherwise of the river or lake nearest the column, or sea level away from them.
    pub water: i32,
    /// Eighths of its top water block the water fills, 1..=8, where water covers the
    /// column: its surface between whole blocks.
    pub top: u8,
    /// The kind of water `water` belongs to.
    pub body: Body,
    /// How river water over the column flows.
    pub current: Option<Current>,
    /// A cave a river runs through under the ground.
    pub cavity: Option<Cavity>,
    /// What the 3D density makes of the column (see `crate::density`): `height` is already
    /// its top; generation hollows the band below it.
    pub crag: Crag,
    /// What the terrain shapes made of the column.
    pub form: Form,
    /// What a trail does here.
    pub trail: Option<crate::trails::TrailMark>,
}

impl Relief {
    fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            elevation: mix(self.elevation, other.elevation),
            continental: mix(self.continental, other.continental),
            mountain: mix(self.mountain, other.mountain),
            peak: mix(self.peak, other.peak),
            roughness: mix(self.roughness, other.roughness),
            valley: mix(self.valley, other.valley),
            crag: mix(self.crag, other.crag),
        }
    }
}

impl GeoSample {
    /// Whether water covers the ground.
    pub fn submerged(&self) -> bool {
        self.water > self.height
    }

    /// Height of the ground the climate cools with: without the towers and spires that
    /// stand on it, and with what rivers cut away.
    pub fn climate_height(&self) -> i32 {
        self.height.min(self.form.base.floor() as i32)
    }

    /// The water in the column, open or in a river's tunnel: the height of the ground or
    /// tunnel floor under it, and of its top block.
    pub fn wet(&self) -> Option<(i32, i32)> {
        match self.cavity {
            Some(cavity) => (self.water > cavity.floor).then_some((cavity.floor, self.water)),
            None => self.submerged().then_some((self.height, self.water)),
        }
    }
}

/// What the terrain shapes made of a column.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Form {
    /// The shape that changes the column most, if any does.
    pub shape: Option<ShapeId>,
    /// How much of that shape's full form rises here, 0 ..= 1.
    pub strength: f32,
    /// Height of the land without the features that stand on it (towers, spires): what the
    /// climate cools with. Volcanoes, mesas, dunes and canyons are part of it.
    pub base: f32,
    /// How high a window opens over `base` under the features, in whole blocks (an arch's).
    pub window: f32,
}

/// The land before rivers and lakes cut into it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Landform {
    /// Height of the ground, in blocks (the top ground block is its whole part).
    pub height: f32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
    /// How far the 3D density moves the ground here, in blocks (see `crate::density`).
    pub crag: f32,
    pub form: Form,
}

/// The land's large-scale relief at a position, from the router (`crate::router`): what
/// the climate, the terrain shapes and the rest of generation read, before shapes, rivers or
/// lakes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Relief {
    /// Height of the land, in blocks.
    pub elevation: f32,
    /// Continentalness, in Tectonic's units: below `COAST` is sea, rising inland.
    pub continental: f32,
    /// 0 off the ranges ..= 1 on their spines.
    pub mountain: f32,
    /// 0 ..= 1 along the highest, craggiest crests.
    pub peak: f32,
    /// 0 on smooth lowland ..= 1 in rugged hill country.
    pub roughness: f32,
    /// 1 on the floor of a valley of the ridge network, falling to 0 beyond its sides.
    pub valley: f32,
    /// How far the 3D density moves the ground here, in blocks (see `crate::density`).
    pub crag: f32,
}

/// Lattice tiles a thread used last, by world and tile.
type Recent<T> = RefCell<Vec<(u64, IVec2, Arc<[T]>)>>;

/// Spacing of the lattice the relief is worked out on, in blocks.
pub const RELIEF_CELL: i32 = 4;

/// Where continentalness reaches the coast.
pub const COAST: f32 = -0.16;
/// Regional climate tiles, in lattice cells a side: large, as each point's climate reads
/// the relief around it, which the points of the same tile share.
pub(crate) const TILE: i32 = 16;
/// Base climate tiles, in lattice cells a side.
const BASE_TILE: i32 = 8;
/// Spacing of the climate's base lattice (sea-level temperature and precipitation, which
/// the land's regions read), in blocks.
pub const BASE_CELL: i32 = 256;

/// Relief already worked out at points of the lattice, by lattice position, for
/// `Geology::reliefs_with`.
#[derive(Default)]
pub struct Corners {
    index: FxHashMap<(i32, i32), u32>,
    values: Vec<Relief>,
}

impl Corners {
    pub(crate) fn len(&self) -> usize { self.values.len() }
}

pub struct Geology {
    router: Router,
    /// The 3D density the land's crags take.
    density: Density,
    /// The world's scale for heights above the sea, from its mountain height.
    vertical: f32,
    pub(crate) climate: Climate,
    shapes: Shapes,
    /// The regional climate at the lattice points of each tile, plus the next row and
    /// column, so any position interpolates within one tile.
    lattice: Cache<IVec2, Arc<[Regional]>>,
    /// The relief at the regional lattice's points, in tiles of `TILE` a side: each point's
    /// climate reads the relief of the points around it, which neighbouring tiles share.
    lattice_reliefs: Cache<IVec2, Arc<[Relief]>>,
    /// The climate's base lattice, in tiles the same way.
    bases: Cache<IVec2, Arc<[Base]>>,
    /// Tells this world's tiles apart in the per-thread memo.
    id: u64,
}

pub fn noise(seed: u64, salt: u32, frequency: f32, fractal: FractalType, octaves: i32) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed((crate::rng::mix(seed ^ salt as u64) >> 32) as i32);
    n.set_noise_type(Some(NoiseType::OpenSimplex2));
    n.set_frequency(Some(frequency));
    n.set_fractal_type(Some(fractal));
    n.set_fractal_octaves(Some(octaves));
    n
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Like `smoothstep`, with a flat first and second derivative at both ends (quintic), so
/// shapes faded by it meet the land around them without a crease.
pub fn smootherstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}

/// Piecewise-linear curve through `(x, y)` points sorted by x.
pub fn spline(points: &[(f32, f32)], x: f32) -> f32 {
    debug_assert!(points.windows(2).all(|w| w[0].0 < w[1].0), "spline points out of order");
    // The first point past x. The points are sorted, so it is the number of points at or
    // before x, which counts without a branch to mispredict. NaN lies past every point.
    let i = if x.is_nan() { points.len() } else { points.iter().filter(|&&(px, _)| px <= x).count() };
    match i {
        0 => points[0].1,
        i if i == points.len() => points[i - 1].1,
        i => {
            let (x0, y0) = points[i - 1];
            let (x1, y1) = points[i];
            y0 + (y1 - y0) * (x - x0) / (x1 - x0)
        }
    }
}

impl Geology {
    pub fn new(seed: u64, dials: &WorldDials, content: &WorldContent, rocks: &Rocks) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let program = Program::compile(&content.land).expect("land.ron is checked at load");
        Self {
            router: Router::new(seed, dials, &program),
            density: Density::new(seed, &content.land.density),
            vertical: 0.6 + 0.4 * dials.mountain_height / 140.0,
            climate: Climate::new(seed, dials, &content.climate),
            shapes: Shapes::new(seed, dials, content, rocks),
            lattice: Cache::new(16 << 20),
            lattice_reliefs: Cache::new(8 << 20),
            bases: Cache::new(4 << 20),
            id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// The climate over this land.
    pub fn climate(&self) -> &Climate {
        &self.climate
    }

    /// The terrain shapes of this land.
    pub fn shapes(&self) -> &Shapes {
        &self.shapes
    }

    /// The relief at (x, z). Generation asks through `reliefs` and `land`; this is for
    /// tests and benchmarks.
    #[cfg(test)]
    pub fn relief(&self, x: f32, z: f32) -> Relief {
        self.reliefs(&[(x, z)])[0]
    }

    /// The relief at each position: bilinear between the points of a lattice every
    /// `RELIEF_CELL` blocks (as Minecraft interpolates its terrain over 4-block cells), each
    /// lattice point worked out once per call, `LANES` at a time, side by side.
    /// Positions on the lattice (the rivers' and trails' grids) take its points as they are.
    pub fn reliefs(&self, points: &[(f32, f32)]) -> Vec<Relief> {
        let cell = RELIEF_CELL as f32;
        // Grids on the lattice, as rivers and trails ask for, need no corners worked out.
        if points.iter().all(|&(x, z)| x % cell == 0.0 && z % cell == 0.0) {
            let mut lanes = self.router.lanes();
            let mut out = Vec::with_capacity(points.len());
            for some in points.chunks(LANES) {
                out.extend_from_slice(&self.relief_lanes(&mut lanes, some)[..some.len()]);
            }
            return out;
        }
        self.reliefs_with(points, &mut Corners::default())
    }

    /// The relief at each position, like `reliefs`, keeping the lattice points worked out in
    /// `corners` and taking those already there: for callers whose positions overlap from
    /// call to call, such as the points of a grid and then the columns beside each.
    pub fn reliefs_with(&self, points: &[(f32, f32)], corners: &mut Corners) -> Vec<Relief> {
        let cell = RELIEF_CELL as f32;
        let Corners { index, values } = corners;
        let mut fresh: Vec<(i32, i32)> = Vec::with_capacity(points.len() + 8);
        let mut refs: Vec<([u32; 4], f32, f32)> = Vec::with_capacity(points.len());
        for &(x, z) in points {
            let (gx, gz) = (x / cell, z / cell);
            let (fx, fz) = (gx.floor(), gz.floor());
            let (tx, tz) = (gx - fx, gz - fz);
            let (cx, cz) = (fx as i32, fz as i32);
            let mut corner = |dx: i32, dz: i32| {
                let key = (cx + dx, cz + dz);
                *index.entry(key).or_insert_with(|| {
                    fresh.push(key);
                    (values.len() + fresh.len() - 1) as u32
                })
            };
            // Only the corners that weigh anything.
            let a = corner(0, 0);
            let b = if tx > 0.0 { corner(1, 0) } else { a };
            let c = if tz > 0.0 { corner(0, 1) } else { a };
            let d = if tx > 0.0 && tz > 0.0 { corner(1, 1) } else if tx > 0.0 { b } else { c };
            refs.push(([a, b, c, d], tx, tz));
        }
        if !fresh.is_empty() {
            // A warm corner lookup needs interpolation only, not fresh router state.
            let mut lanes = self.router.lanes();
            values.reserve(fresh.len());
            for some in fresh.chunks(LANES) {
                let at: [(f32, f32); LANES] = std::array::from_fn(|k| {
                    let (cx, cz) = some[k.min(some.len() - 1)];
                    (cx as f32 * cell, cz as f32 * cell)
                });
                values.extend_from_slice(&self.relief_lanes(&mut lanes, &at[..some.len()])[..some.len()]);
            }
        }
        refs.iter()
            .map(|&([a, b, c, d], tx, tz)| {
                let at = |i: u32| values[i as usize];
                at(a).lerp(at(b), tx).lerp(at(c).lerp(at(d), tx), tz)
            })
            .collect()
    }

    /// The relief at one to `LANES` positions side by side, exactly as at each alone. Lanes
    /// past the positions are left at their defaults.
    fn relief_lanes(&self, lanes: &mut Lanes, points: &[(f32, f32)]) -> [Relief; LANES] {
        let router = &self.router;
        let o = router.outputs;
        router.start(lanes, points);
        let mut out = [Relief::default(); LANES];
        for (k, out) in out.iter_mut().enumerate().take(points.len()) {
            let mut get = |field| router.get(lanes, self, field, k);
            let surface = get(o.surface);
            *out = Relief {
                elevation: SEA_LEVEL as f32 + router.height(surface, self.vertical),
                continental: get(o.continental),
                mountain: get(o.mountain),
                peak: get(o.peak),
                roughness: get(o.roughness),
                valley: get(o.valley),
                crag: o.crag.map_or(0.0, |crag| get(crag).max(0.0)),
            };
        }
        out
    }

    /// Continentalness and the mountain field at each position, which read no climate:
    /// what the climate's rain shadows are made from.
    pub(crate) fn shadows(&self, points: &[(f32, f32)]) -> Vec<(f32, f32)> {
        let router = &self.router;
        let o = router.outputs;
        let mut lanes = router.lanes();
        let mut out = Vec::with_capacity(points.len());
        for some in points.chunks(LANES) {
            router.start(&mut lanes, some);
            for k in 0..some.len() {
                let c = router.get(&mut lanes, &NoClimate, o.continental, k);
                out.push((c, router.get(&mut lanes, &NoClimate, o.mountain, k)));
            }
        }
        out
    }

    /// The router's continentalness at each position, exactly there rather than from the
    /// lattice: what puts a biome on the right side of the coast.
    pub(crate) fn continentals(&self, points: &[(f32, f32)]) -> Vec<f32> {
        let router = &self.router;
        let mut lanes = router.lanes();
        let mut out = Vec::with_capacity(points.len());
        for some in points.chunks(LANES) {
            router.start(&mut lanes, some);
            out.extend((0..some.len()).map(|k| router.get(&mut lanes, &NoClimate, router.outputs.continental, k)));
        }
        out
    }

    /// The 3D density the land's crags take.
    pub(crate) fn density(&self) -> &Density {
        &self.density
    }

    /// The land at (x, z), before rivers and lakes.
    pub fn land(&self, x: i32, z: i32) -> Landform {
        self.lands(&[(x, z)])[0]
    }

    /// The land at each position: its relief, with the terrain shapes on it.
    pub fn lands(&self, points: &[(i32, i32)]) -> Vec<Landform> {
        let reliefs = self.reliefs(&points.iter().map(|&(x, z)| (x as f32, z as f32)).collect::<Vec<_>>());
        self.shaped(points, &reliefs)
    }

    /// The land at each position, like `lands`, sharing lattice points through `corners`
    /// (see `reliefs_with`).
    pub fn lands_with(&self, points: &[(i32, i32)], corners: &mut Corners) -> Vec<Landform> {
        let reliefs = self.reliefs_with(&points.iter().map(|&(x, z)| (x as f32, z as f32)).collect::<Vec<_>>(), corners);
        self.shaped(points, &reliefs)
    }

    /// One landform with exactly `lands_with`'s corner order and interpolation, keeping
    /// its single point, fresh corners, references and outputs in fixed arrays.
    pub(crate) fn land_with(&self, x: i32, z: i32, corners: &mut Corners) -> Landform {
        let cell = RELIEF_CELL as f32;
        let (gx, gz) = (x as f32 / cell, z as f32 / cell);
        let (fx, fz) = (gx.floor(), gz.floor());
        let (tx, tz) = (gx - fx, gz - fz);
        let (cx, cz) = (fx as i32, fz as i32);
        let Corners { index, values } = corners;
        let mut fresh = [(0, 0); 4];
        let mut count = 0;
        let mut corner = |dx: i32, dz: i32| {
            let key = (cx + dx, cz + dz);
            *index.entry(key).or_insert_with(|| {
                fresh[count] = key;
                count += 1;
                (values.len() + count - 1) as u32
            })
        };
        let a = corner(0, 0);
        let b = if tx > 0.0 { corner(1, 0) } else { a };
        let c = if tz > 0.0 { corner(0, 1) } else { a };
        let d = if tx > 0.0 && tz > 0.0 { corner(1, 1) } else if tx > 0.0 { b } else { c };
        if count > 0 {
            let mut lanes = self.router.lanes();
            values.reserve(count);
            let at: [(f32, f32); 4] = std::array::from_fn(|k| {
                let (cx, cz) = fresh[k.min(count - 1)];
                (cx as f32 * cell, cz as f32 * cell)
            });
            values.extend_from_slice(&self.relief_lanes(&mut lanes, &at[..count])[..count]);
        }
        let at = |i: u32| values[i as usize];
        let relief = at(a).lerp(at(b), tx).lerp(at(c).lerp(at(d), tx), tz);
        let mut out = [Landform::default()];
        self.shaped_lanes(&[(x, z)], &[relief], &mut out);
        out[0]
    }

    /// The land at each position with the relief given there.
    fn shaped(&self, points: &[(i32, i32)], reliefs: &[Relief]) -> Vec<Landform> {
        let mut out = vec![Landform::default(); points.len()];
        for ((four, reliefs), out) in points.chunks(4).zip(reliefs.chunks(4)).zip(out.chunks_mut(4)) {
            self.shaped_lanes(four, reliefs, out);
        }
        out
    }

    /// The land at one to four positions with the relief given: the terrain shapes on it.
    fn shaped_lanes(&self, points: &[(i32, i32)], reliefs: &[Relief], out: &mut [Landform]) {
        let n = points.len();
        let anywhere = |x: i32, z: i32| self.regional(x, z);
        let grounds: [Ground; 4] = std::array::from_fn(|k| {
            let relief = &reliefs[k.min(n - 1)];
            Ground { regional: &anywhere, height: relief.elevation, hills: 0.0, relief }
        });
        let regionals: [Regional; 4] = std::array::from_fn(|k| if k < n { self.regional(points[k].0, points[k].1) } else { Regional::default() });
        let shaped = self.shapes.shape_lanes(points, &grounds[..n], &regionals[..n]);
        let sea = SEA_LEVEL as f32;
        let ceiling = self.router.ceiling();
        for (k, out) in out.iter_mut().enumerate() {
            let (ground, shaped) = (grounds[k].height, &shaped[k]);
            let base = sea + ease(ground + shaped.terrain - sea, ceiling);
            let height = sea + ease(ground + shaped.terrain + shaped.features - sea, ceiling);
            let form = Form { shape: shaped.shape, strength: shaped.strength, base, window: shaped.window };
            *out = Landform { height, mountain: reliefs[k].mountain, crag: reliefs[k].crag, form };
        }
    }

    /// The climate's base fields at (x, z): sea-level temperature and precipitation,
    /// bilinear between the points of its lattice.
    pub fn base(&self, x: f32, z: f32) -> Base {
        let cell = BASE_CELL as f32;
        let (gx, gz) = (x / cell, z / cell);
        let (fx, fz) = (gx.floor(), gz.floor());
        let point = IVec2::new(fx as i32, fz as i32);
        let key = point.div_euclid(IVec2::splat(BASE_TILE));
        let local = point - key * BASE_TILE;
        let (tx, tz) = (gx - fx, gz - fz);
        self.with_base_tile(key, |tile| {
            let at = |i: i32, j: i32| tile[((local.y + j) * (BASE_TILE + 1) + local.x + i) as usize];
            at(0, 0).lerp(at(1, 0), tx).lerp(at(0, 1).lerp(at(1, 1), tx), tz)
        })
    }

    /// The base climate at one to four positions, from one tile of its lattice when they
    /// share one, as neighbouring positions do.
    fn bases_at(&self, x: [f32; 4], z: [f32; 4], n: usize) -> [Base; 4] {
        let cell = BASE_CELL as f32;
        let at: [(IVec2, f32, f32); 4] = std::array::from_fn(|k| {
            let (gx, gz) = (x[k.min(n - 1)] / cell, z[k.min(n - 1)] / cell);
            let (fx, fz) = (gx.floor(), gz.floor());
            (IVec2::new(fx as i32, fz as i32), gx - fx, gz - fz)
        });
        let key = at[0].0.div_euclid(IVec2::splat(BASE_TILE));
        if at.iter().all(|a| a.0.div_euclid(IVec2::splat(BASE_TILE)) == key) {
            return self.with_base_tile(key, |tile| {
                at.map(|(point, tx, tz)| {
                    let local = point - key * BASE_TILE;
                    let at = |i: i32, j: i32| tile[((local.y + j) * (BASE_TILE + 1) + local.x + i) as usize];
                    at(0, 0).lerp(at(1, 0), tx).lerp(at(0, 1).lerp(at(1, 1), tx), tz)
                })
            });
        }
        std::array::from_fn(|k| self.base(x[k.min(n - 1)], z[k.min(n - 1)]))
    }

    /// Calls `f` with a tile of the climate's base lattice, kept at hand per thread like the
    /// regional climate's (see `with_tile`). A tile not at hand is worked out outside the
    /// memo's borrow, since the base lattice reads the land.
    fn with_base_tile<R>(&self, key: IVec2, f: impl FnOnce(&[Base]) -> R) -> R {
        thread_local! {
            static RECENT: Recent<Base> = const { RefCell::new(Vec::new()) };
        }
        let at_hand = RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            let i = recent.iter().position(|(id, k, _)| *id == self.id && *k == key)?;
            if i > 0 {
                let entry = recent.remove(i);
                recent.insert(0, entry);
            }
            Some(())
        });
        if at_hand.is_none() {
            let tile = self.bases.get(key, || {
                let side = BASE_TILE + 1;
                let points: Vec<(f32, f32)> = (0..side * side)
                    .map(|i| {
                        let p = (key * BASE_TILE + IVec2::new(i % side, i / side)) * BASE_CELL;
                        (p.x as f32, p.y as f32)
                    })
                    .collect();
                self.climate.bases(self, &points).into()
            });
            RECENT.with(|recent| {
                let mut recent = recent.borrow_mut();
                recent.insert(0, (self.id, key, tile));
                recent.truncate(4);
            });
        }
        RECENT.with(|recent| f(&recent.borrow()[0].2))
    }

    /// The regional climate at a lattice point (in lattice cells).
    pub fn regional_point(&self, point: IVec2) -> Regional {
        let key = point.div_euclid(IVec2::splat(TILE));
        let local = point - key * TILE;
        self.with_tile(key, |tile| tile[(local.y * (TILE + 1) + local.x) as usize])
    }

    /// The regional climate at (x, z), bilinear between the lattice points around it: the
    /// same fields every sampler of the climate sees there.
    pub fn regional(&self, x: i32, z: i32) -> Regional {
        let cell = IVec2::new(x, z).div_euclid(IVec2::splat(climate::CELL));
        let key = cell.div_euclid(IVec2::splat(TILE));
        let local = cell - key * TILE;
        let f = |v: i32| v.rem_euclid(climate::CELL) as f32 / climate::CELL as f32;
        self.with_tile(key, |tile| {
            let at = |i: i32, j: i32| tile[((local.y + j) * (TILE + 1) + local.x + i) as usize];
            at(0, 0).lerp(at(1, 0), f(x)).lerp(at(0, 1).lerp(at(1, 1), f(x)), f(z))
        })
    }

    /// The relief at points of the regional climate's lattice, from the tiles of it shared
    /// by neighbouring tiles of the climate: the same as `reliefs` there.
    fn lattice_reliefs_at(&self, points: &[(f32, f32)]) -> Vec<Relief> {
        let mut tiles: Vec<(IVec2, Arc<[Relief]>)> = Vec::new();
        points
            .iter()
            .map(|&(x, z)| {
                let point = IVec2::new(x as i32, z as i32).div_euclid(IVec2::splat(climate::CELL));
                debug_assert!(point * climate::CELL == IVec2::new(x as i32, z as i32) && x.fract() == 0.0 && z.fract() == 0.0, "({x}, {z}) off the lattice");
                let key = point.div_euclid(IVec2::splat(TILE));
                let local = point - key * TILE;
                let i = match tiles.iter().position(|(k, _)| *k == key) {
                    Some(i) => i,
                    None => {
                        let tile = self.lattice_reliefs.get(key, || {
                            let points: Vec<(f32, f32)> = (0..TILE * TILE)
                                .map(|i| {
                                    let p = (key * TILE + IVec2::new(i % TILE, i / TILE)) * climate::CELL;
                                    (p.x as f32, p.y as f32)
                                })
                                .collect();
                            self.reliefs(&points).into()
                        });
                        tiles.push((key, tile));
                        tiles.len() - 1
                    }
                };
                tiles[i].1[(local.y * TILE + local.x) as usize]
            })
            .collect()
    }

    /// The tile `key` of the regional climate lattice: its `TILE + 1` points a side, row
    /// by row, `climate::CELL` blocks apart from `key * TILE` lattice cells.
    pub(crate) fn regional_tile(&self, key: IVec2) -> Arc<[Regional]> {
        self.with_tile(key, Arc::clone)
    }

    /// Calls `f` with a tile of the regional climate lattice. The tiles this thread used
    /// last are kept at hand, which spares the shared cache's lock and the shared count of
    /// the tile's owners on most calls.
    fn with_tile<R>(&self, key: IVec2, f: impl FnOnce(&Arc<[Regional]>) -> R) -> R {
        thread_local! {
            static RECENT: Recent<Regional> = const { RefCell::new(Vec::new()) };
        }
        RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            match recent.iter().position(|(id, k, _)| *id == self.id && *k == key) {
                Some(0) => {}
                Some(i) => {
                    let entry = recent.remove(i);
                    recent.insert(0, entry);
                }
                None => {
                    let tile = self.lattice.get(key, || {
                        let side = TILE + 1;
                        let points: Vec<(f32, f32)> = (0..side * side)
                            .map(|i| {
                                let p = (key * TILE + IVec2::new(i % side, i / side)) * climate::CELL;
                                (p.x as f32, p.y as f32)
                            })
                            .collect();
                        self.climate.regionals_with(self, &points, |around| self.lattice_reliefs_at(around)).into()
                    });
                    recent.insert(0, (self.id, key, tile));
                    recent.truncate(6);
                }
            }
            f(&recent[0].2)
        })
    }
}

/// Geology already sampled over a square of positions, which falls back to sampling
/// anywhere else (and keeps those samples, since decorations reaching out of the column
/// ask for the same few positions over and over).
pub struct GeoGrid<'a> {
    world: &'a Overworld,
    min: (i32, i32),
    size: i32,
    samples: Vec<GeoSample>,
    beyond: RefCell<FxHashMap<(i32, i32), GeoSample>>,
}

impl<'a> GeoGrid<'a> {
    /// Samples the `size`×`size` positions from `min`.
    pub fn new(world: &'a Overworld, min: (i32, i32), size: i32) -> Self {
        let samples = world.geo_area(min.0, min.1, size);
        Self { world, min, size, samples, beyond: RefCell::default() }
    }

    pub fn samples(&self) -> &[GeoSample] {
        &self.samples
    }

    pub fn get(&self, x: i32, z: i32) -> GeoSample {
        let (i, j) = (x - self.min.0, z - self.min.1);
        if (0..self.size).contains(&i) && (0..self.size).contains(&j) {
            self.samples[(j * self.size + i) as usize]
        } else {
            *self.beyond.borrow_mut().entry((x, z)).or_insert_with(|| self.world.geo(x, z))
        }
    }

    /// The height of the ground at (x, z), without copying the rest of its sample.
    fn height(&self, x: i32, z: i32) -> i32 {
        let (i, j) = (x - self.min.0, z - self.min.1);
        if (0..self.size).contains(&i) && (0..self.size).contains(&j) { self.samples[(j * self.size + i) as usize].height } else { self.get(x, z).height }
    }

    /// How steep the ground is at (x, z) (see `Site::steepness`).
    pub fn steepness(&self, x: i32, z: i32) -> f32 {
        crate::steepness(|dx, dz| self.height(x + dx, z + dz))
    }

    /// Steepest height step from (x, z) to its four neighbours, in blocks.
    pub fn slope(&self, x: i32, z: i32) -> i32 {
        let height = self.height(x, z);
        [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().map(|(dx, dz)| (self.height(x + dx, z + dz) - height).abs()).max().unwrap_or(0)
    }
}

/// Eases heights above the sea that pass `knee` towards `knee + room`.
fn ease(above: f32, (knee, room): (f32, f32)) -> f32 {
    if above > knee { knee + room * ((above - knee) / room).tanh() } else { above }
}

impl crate::router::Inputs for Geology {
    /// `T` and `H` for the router: the base climate's sea-level temperature and moisture.
    fn climate(&self, x: [f32; 4], z: [f32; 4], n: usize) -> ([f32; 4], [f32; 4]) {
        let bases = self.bases_at(x, z, n);
        (bases.map(|b| b.sea_temperature), bases.map(|b| b.humidity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_land_buffers_keep_original_corners_and_all_facts() {
        let registry = std::sync::Arc::new(crate::test_registry());
        let land_bits = |l: Landform| (l.height.to_bits(), l.mountain.to_bits(), l.crag.to_bits(),
            l.form.shape, l.form.strength.to_bits(), l.form.base.to_bits(), l.form.window.to_bits());
        let relief_bits = |r: &Relief| [r.elevation.to_bits(), r.continental.to_bits(), r.mountain.to_bits(),
            r.peak.to_bits(), r.roughness.to_bits(), r.valley.to_bits(), r.crag.to_bits()];
        let mut points: Vec<_> = (-5..=5).flat_map(|z| (-5..=5).map(move |x| (x, z))).collect();
        points.extend([(-113, -97), (65_537, -65_535), (16_777_215, -16_777_215),
            (16_777_216, -16_777_216), (16_777_217, -16_777_217)]);
        for seed in [7, 42, 7] {
            let world = crate::Overworld::new(seed, registry.clone());
            let geology = &world.geology;
            for warm in [0, points.len() / 2, points.len()] {
                let (mut original, mut scalar) = (Corners::default(), Corners::default());
                for &p in points.iter().take(warm) {
                    geology.lands_with(&[p], &mut original);
                    geology.lands_with(&[p], &mut scalar);
                }
                let before = scalar.len();
                for &(x, z) in points.iter().chain(points.iter().rev()) {
                    let expected = geology.lands_with(&[(x, z)], &mut original)[0];
                    let actual = geology.land_with(x, z, &mut scalar);
                    assert_eq!(land_bits(actual), land_bits(expected), "seed {seed} at {x},{z}, warm {warm}");
                    assert_eq!(scalar.index, original.index, "exact corner keys and insertion indices");
                    assert_eq!(scalar.values.iter().map(relief_bits).collect::<Vec<_>>(),
                        original.values.iter().map(relief_bits).collect::<Vec<_>>(), "every complete relief field in original order");
                    assert_eq!(scalar.values.capacity(), original.values.capacity(), "same persistent corner allocation policy");
                }
                if warm == points.len() { assert_eq!(scalar.len(), before, "wholly warm lookups add no corners"); }
                else { assert!(scalar.len() > before, "cold and partially warm fixtures actually fill corners"); }
            }
        }
    }

    /// Land worked out many positions at a time is exactly the land at each alone, over
    /// sea, coast, plains and ranges, in any grouping.
    #[test]
    fn lands_match_land_alone() {
        use rayon::prelude::*;
        let registry = std::sync::Arc::new(crate::test_registry());
        // Each seed, and each group, on its own core.
        [7, 42].into_par_iter().for_each(|seed| {
            let world = crate::Overworld::new(seed, registry.clone());
            let geology = &world.geology;
            let mut rng = crate::rng::Rng::new(seed, 1, 2, 3);
            // Rows of a grid, so that neighbouring lanes are alike, and scattered positions,
            // so that they are not; seven at a time leaves groups of four and of three.
            let mut points: Vec<(i32, i32)> = (0..4000).map(|i| (i % 80 * 37 - 1500, i / 80 * 41 - 1000)).collect();
            points.extend((0..4000).map(|_| (rng.range(-20_000, 20_000), rng.range(-20_000, 20_000))));
            points.par_chunks(7).for_each(|group| {
                let lands = geology.lands(group);
                let floats: Vec<(f32, f32)> = group.iter().map(|&(x, z)| (x as f32 + 0.5, z as f32 - 0.25)).collect();
                let reliefs = geology.reliefs(&floats);
                for (k, &(x, z)) in group.iter().enumerate() {
                    assert_eq!(format!("{:?}", lands[k]), format!("{:?}", geology.land(x, z)), "land at ({x}, {z})");
                    assert_eq!(format!("{:?}", reliefs[k]), format!("{:?}", geology.relief(floats[k].0, floats[k].1)), "relief at {:?}", floats[k]);
                }
            });
        });
    }

    /// The regional climate worked out four positions at a time is exactly the climate at
    /// each alone.
    #[test]
    fn regionals_match_regional_alone() {
        let registry = std::sync::Arc::new(crate::test_registry());
        let world = crate::Overworld::new(3, registry);
        let mut rng = crate::rng::Rng::new(3, 4, 5, 6);
        let points: Vec<(f32, f32)> = (0..2000).map(|i| if i % 2 == 0 { ((i % 40 * 64) as f32, (i / 40 * 64) as f32) } else { (rng.range(-30_000, 30_000) as f32, rng.range(-30_000, 30_000) as f32) }).collect();
        for group in points.chunks(7) {
            let batch = world.geology.climate.regionals(&world.geology, group);
            for (k, &(x, z)) in group.iter().enumerate() {
                assert_eq!(format!("{:?}", batch[k]), format!("{:?}", world.geology.climate.regional(&world.geology, x, z)), "at ({x}, {z})");
            }
        }
    }

    /// A climate grid worked out point by point reads as the one from the cached tiles.
    #[test]
    fn a_grid_alone_matches_the_tiles() {
        let registry = std::sync::Arc::new(crate::test_registry());
        let world = crate::Overworld::new(3, registry);
        let mut rng = crate::rng::Rng::new(3, 4, 5, 7);
        for _ in 0..40 {
            let min = IVec2::new(rng.range(-30_000, 30_000), rng.range(-30_000, 30_000));
            let max = min + IVec2::new(rng.range(0, 300), rng.range(0, 300));
            let (tiles, alone) = (climate::ClimateGrid::new(&world.geology, min, max), climate::ClimateGrid::alone(&world.geology, min, max));
            for (x, z) in [(min.x, min.y), (max.x, max.y), ((min.x + max.x) / 2, (min.y + max.y) / 2)] {
                assert_eq!(format!("{:?}", tiles.at(x, z)), format!("{:?}", alone.at(x, z)), "at ({x}, {z})");
            }
        }
    }

    /// `cargo test --release -p voxmar_worldgen land_timing -- --ignored --nocapture` prints
    /// the time the relief, the land with its shapes, and the regional climate take per
    /// position, in CPU time on one thread.
    #[test]
    #[ignore]
    fn land_timing() {
        let registry = crate::test_registry();
        let world = crate::Overworld::new(7, std::sync::Arc::new(registry));
        let geology = &world.geology;
        let points: Vec<(i32, i32)> = (0..200_000).map(|i| (i % 400 * 3 - 600, i / 400 * 3 - 750)).collect();
        // Warm the climate lattice first.
        for &(x, z) in points.iter().step_by(97) {
            std::hint::black_box(geology.land(x, z));
        }
        let time = |name: &str, f: &dyn Fn(i32, i32) -> f32| {
            let start = crate::perf::thread_time();
            let mut sum = 0.0;
            for &(x, z) in &points {
                sum += f(x, z);
            }
            std::hint::black_box(sum);
            println!("{name:10} {:.0} ns", (crate::perf::thread_time() - start).as_nanos() as f64 / points.len() as f64);
        };
        time("relief", &|x, z| geology.relief(x as f32, z as f32).elevation);
        time("regional", &|x, z| geology.regional(x, z).rainfall);
        time("shapes", &|x, z| {
            let relief = geology.relief(x as f32, z as f32);
            let anywhere = |x: i32, z: i32| geology.regional(x, z);
            let ground = Ground { regional: &anywhere, height: relief.elevation, hills: 0.0, relief: &relief };
            geology.shapes.shape(x, z, &ground, &geology.regional(x, z)).terrain
        });
        time("land", &|x, z| geology.land(x, z).height);
        // Batched, as rivers, trails and the climate ask.
        let floats: Vec<(f32, f32)> = points.iter().map(|&(x, z)| (x as f32, z as f32)).collect();
        let start = crate::perf::thread_time();
        let mut sum = 0.0;
        for chunk in floats.chunks(256) {
            sum += geology.reliefs(chunk).iter().map(|r| r.elevation).sum::<f32>();
        }
        std::hint::black_box(sum);
        println!("reliefs    {:.0} ns per position, batched", (crate::perf::thread_time() - start).as_nanos() as f64 / points.len() as f64);
        let start = crate::perf::thread_time();
        let mut sum = 0.0;
        for chunk in points.chunks(256) {
            sum += geology.lands(chunk).iter().map(|l| l.height).sum::<f32>();
        }
        std::hint::black_box(sum);
        println!("lands      {:.0} ns per position, batched", (crate::perf::thread_time() - start).as_nanos() as f64 / points.len() as f64);
        let start = crate::perf::thread_time();
        for &(x, z) in points.iter().step_by(10) {
            std::hint::black_box(geology.climate.regional(geology, x as f32, z as f32));
        }
        println!("climate    {:.0} ns per regional point", (crate::perf::thread_time() - start).as_nanos() as f64 / (points.len() / 10) as f64);
    }

    /// `cargo test -p voxmar_worldgen noise_spreads -- --ignored --nocapture` prints how the
    /// relief noises are spread, for setting the constants above.
    #[test]
    #[ignore]
    fn noise_spreads() {
        let seed = 7;
        let fields: [(&str, FastNoiseLite); 6] = [
            ("continental", noise(seed, 2, 1.0 / 4000.0, FractalType::FBm, 5)),
            ("ranges", noise(seed, 60, 1.0 / 3400.0, FractalType::FBm, 3)),
            ("range strength", noise(seed, 61, 1.0 / 2400.0, FractalType::FBm, 2)),
            ("ridges", noise(seed, 4, 1.0 / 520.0, FractalType::Ridged, 5)),
            ("plateaus", noise(seed, 63, 1.0 / 1700.0, FractalType::FBm, 2)),
            ("valleys", noise(seed, 64, 1.0 / 1300.0, FractalType::FBm, 2)),
        ];
        for (name, n) in fields {
            let mut values: Vec<f32> = (0..200 * 200).map(|i| n.get_noise_2d((i % 200) as f32 * 97.0, (i / 200) as f32 * 97.0)).collect();
            values.sort_by(f32::total_cmp);
            let mean = values.iter().sum::<f32>() / values.len() as f32;
            let sd = (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt();
            let q = |p: f32| values[((values.len() - 1) as f32 * p) as usize];
            let folded = |w: f32| values.iter().filter(|v| v.abs() < w).count() as f32 / values.len() as f32;
            // Slope near the zero line, in noise units per block.
            let mut slopes: Vec<f32> = (0..20000)
                .filter_map(|i| {
                    let (x, z) = ((i % 141) as f32 * 131.0, (i / 141) as f32 * 131.0);
                    let v = n.get_noise_2d(x, z);
                    (v.abs() < 0.03).then(|| (n.get_noise_2d(x + 1.0, z) - v).hypot(n.get_noise_2d(x, z + 1.0) - v))
                })
                .collect();
            slopes.sort_by(f32::total_cmp);
            let slope = slopes.get(slopes.len() / 2).copied().unwrap_or(0.0);
            println!(
                "{name:15} mean {mean:6.3} sd {sd:5.3}  q01 {:6.3} q50 {:6.3} q99 {:6.3}  |v|<0.1 {:4.2} |v|<0.2 {:4.2}  slope at 0 {slope:.5}/block",
                q(0.01),
                q(0.5),
                q(0.99),
                folded(0.1),
                folded(0.2)
            );
        }
    }
}


#[cfg(test)]
mod profile {
    /// `FIELD=C RELIEF_REPEAT=n cargo test --release -p voxmar_worldgen field_profile -- --ignored`
    /// works out one field of the land over the benchmark's points, for counting what each
    /// costs.
    #[test]
    #[ignore]
    fn field_profile() {
        let registry = crate::test_registry();
        let world = crate::Overworld::new(7, std::sync::Arc::new(registry));
        let spawn = crate::Generator::spawn_point(&world);
        let points: Vec<(f32, f32)> = (0..200_000).map(|i| ((spawn.x + i % 500 * 8 - 2000) as f32, (spawn.z + i / 500 * 8 - 1600) as f32)).collect();
        let router = &world.geology.router;
        let field = router.field(&std::env::var("FIELD").unwrap_or("C".into())).expect("no such field");
        let repeat: u32 = std::env::var("RELIEF_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let mut lanes = router.lanes();
        let mut sum = 0.0;
        for _ in 0..repeat {
            for four in points.chunks(4) {
                router.start(&mut lanes, four);
                for k in 0..4 {
                    sum += router.get(&mut lanes, &world.geology, field, k);
                }
            }
        }
        std::hint::black_box(sum);
    }

    /// `cargo test --release -p voxmar_worldgen relief_profile -- --ignored` samples the relief
    /// in batches for a while, for a profiler to attach to.
    #[test]
    #[ignore]
    fn relief_profile() {
        let registry = crate::test_registry();
        let world = crate::Overworld::new(7, std::sync::Arc::new(registry));
        // Rows of an 8-block grid on land around spawn, as rivers and trails ask for it.
        let spawn = crate::Generator::spawn_point(&world);
        let points: Vec<(f32, f32)> = (0..200_000).map(|i| ((spawn.x + i % 500 * 8 - 2000) as f32, (spawn.z + i / 500 * 8 - 1600) as f32)).collect();
        // `RELIEF_REPEAT=n` repeats the batches n times instead, for counting instructions
        // with `/usr/bin/time -l` (which, unlike time, a busy machine does not change).
        let repeat: Option<u32> = std::env::var("RELIEF_REPEAT").ok().and_then(|v| v.parse().ok());
        let start = std::time::Instant::now();
        let mut sum = 0.0;
        let mut passes = 0;
        while repeat.map_or(start.elapsed().as_secs() < 20, |n| passes < n) {
            passes += 1;
            for chunk in points.chunks(500) {
                sum += world.geology.reliefs(chunk).iter().map(|r| r.elevation).sum::<f32>();
            }
        }
        std::hint::black_box(sum);
    }
}
