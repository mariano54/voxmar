//! The land's router: a few large noise fields, and the fields derived from them through
//! nested splines, from which the terrain's height (and, later, how three-dimensional the
//! ground is) and the parameters biomes are picked by are all read.
//!
//! This is the architecture of Minecraft 1.18's noise router, and of Tectonic, which
//! rewrote its inputs and splines (see INSPIRATIONS.md and THIRD_PARTY_NOTICES.md):
//! continentalness, erosion and ridges define a point in parameter space at every position,
//! and cubic Hermite splines whose knot values may themselves be splines turn that point
//! into the height of the land. Because terrain, biomes and terrain features all read the
//! same few numbers, a mountain biome lies on the mountain and a valley biome in the valley,
//! and nothing has a shape of its own that could look stamped (`docs/terrain-foundation.md`).
//!
//! Everything is data (`land.ron`): the noises (wavelength, the weight of each octave,
//! spread, warp), and the derived fields as small expressions: splines, sums, products,
//! `abs` and the like over noises, earlier fields and a few built-in inputs:
//!
//! - `T`: the regional air temperature at sea level, °C, and `H`, the regional moisture
//!   (precipitation over evaporation) at sea level, both smooth over kilometres (see
//!   `crate::climate`);
//! - `continents_fold`: the `continents` noise folded with `abs()` and divided by its value
//!   at the coast for the world's ocean fraction: 0 along the middle of the seas, 1 at the
//!   coast, larger inland;
//! - world dials as constants: `tectonic` (activity), `young` (1 − the land's age).
//!
//! Fields are evaluated for eight positions side by side (two groups of the four lanes of
//! `crate::noise`), lazily and each only once per position: a spline evaluates only the knots beside its
//! coordinate, in the lanes that need them, and a product stops at its first zero factor,
//! so a gate in front spares the work behind it.

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType, NoiseType};

use crate::content::{ContentError, ExprDef, LandDef, OpDef};
use crate::dials::WorldDials;

/// Built-in inputs, the first slots.
const T: u16 = 0;
const H: u16 = 1;
const FOLD: u16 = 2;
const INPUTS: [&str; 3] = ["T", "H", "continents_fold"];
/// Most knots a spline may have.
const MAX_KNOTS: usize = 64;
/// Mean continent size the `continent_size` dial is measured against, in blocks.
const CONTINENT_SIZE: f32 = 2300.0;

type NodeId = u32;

/// Positions evaluated side by side: two groups of `crate::noise`'s four lanes, so each
/// step of the program is worked out once for eight positions.
pub(crate) const LANES: usize = 8;
/// A value in each lane.
type Wide = [f32; LANES];

/// Lanes `4 g..4 g + 4` of `v`.
fn quarter(v: &Wide, g: usize) -> [f32; 4] {
    std::array::from_fn(|k| v[4 * g + k])
}

/// The groups of four lanes that `mask` has any lane of.
fn groups(mask: u8) -> impl Iterator<Item = usize> {
    (0..LANES / 4).filter(move |g| mask & (0xf << (4 * g)) != 0)
}

/// The lanes of the groups of four that `mask` has any lane of.
fn whole_groups(mask: u8) -> u8 {
    groups(mask).fold(0, |m, g| m | 0xf << (4 * g))
}

#[derive(Clone, Copy, Debug)]
enum Node {
    Const(f32),
    Slot(u16),
    /// `leaf` when every knot's value is a number.
    Spline { on: NodeId, first: u32, count: u32, leaf: bool },
    Add(u32, u32),
    Mul(u32, u32),
    Min(u32, u32),
    Max(u32, u32),
    Abs(NodeId),
    Square(NodeId),
    Clamp(NodeId, f32, f32),
    Shifted { noise: u16, dx: NodeId, dz: NodeId },
}

#[derive(Clone, Copy, Debug)]
struct Knot {
    at: f32,
    value: NodeId,
    slope: f32,
    /// The value, when it is a number rather than an expression.
    constant: Option<f32>,
}

/// The fields the generator reads by name.
#[derive(Clone, Copy, Debug)]
pub struct Outputs {
    /// Height of the land above the sea in units of 128 blocks, before the vertical scale.
    pub surface: Field,
    /// Continentalness, in Tectonic's units: the coast lies near `crate::geology::COAST`.
    pub continental: Field,
    /// 0 off the mountain ranges ..= 1 on their spines.
    pub mountain: Field,
    /// 0 ..= 1 along the highest, craggiest crests.
    pub peak: Field,
    /// 0 on smooth lowland ..= 1 in rugged hill country.
    pub roughness: Field,
    /// 1 on the floor of a valley of the ridge network, falling to 0 beyond its sides.
    pub valley: Field,
    /// How far the 3D density moves the ground, in blocks (see `crate::density`), where the
    /// land defines it.
    pub crag: Option<Field>,
}

/// A field, by its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field(u16);

/// The structure of a land, checked and compiled, before any noise is seeded.
#[derive(Clone, Debug)]
pub struct Program {
    nodes: Vec<Node>,
    knots: Vec<Knot>,
    args: Vec<NodeId>,
    noises: Vec<(String, crate::content::LandNoiseDef)>,
    warps: Vec<(String, crate::content::LandWarpDef)>,
    /// The root of each derived field, and whether it reads the climate.
    fields: Vec<(String, NodeId, bool)>,
    /// Dial constants, patched at seeding: (node, dial).
    dials: Vec<(NodeId, Dial)>,
    outputs: Outputs,
    vertical_scale: f32,
    elevation_boost: f32,
    ceiling: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
enum Dial {
    Tectonic,
    Young,
}

impl Program {
    /// Compiles `land.ron`, checking every name, the order of knots and that the fields the
    /// generator reads exist.
    pub fn compile(def: &LandDef) -> Result<Self, ContentError> {
        let err = |what: String| ContentError(format!("land.ron: {what}"));
        let mut program = Program {
            nodes: Vec::new(),
            knots: Vec::new(),
            args: Vec::new(),
            noises: def.noises.clone(),
            warps: def.warps.clone(),
            fields: Vec::new(),
            dials: Vec::new(),
            outputs: Outputs {
                surface: Field(0),
                continental: Field(0),
                mountain: Field(0),
                peak: Field(0),
                roughness: Field(0),
                valley: Field(0),
                crag: None,
            },
            vertical_scale: def.vertical_scale,
            elevation_boost: def.elevation_boost,
            ceiling: def.ceiling,
        };
        for (name, noise) in &def.noises {
            if noise.octaves.is_empty() || noise.octaves.iter().all(|&w| w == 0.0) || noise.wavelength <= 0.0 || noise.spread <= 0.0 {
                return Err(err(format!("noise `{name}` needs a positive wavelength and spread and an octave of some weight")));
            }
            if let Some(warp) = &noise.warp
                && !def.warps.iter().any(|(w, _)| w == warp)
            {
                return Err(err(format!("noise `{name}`: unknown warp `{warp}`")));
            }
            if let Some(dial) = &noise.scale_by
                && dial != "continent_size"
            {
                return Err(err(format!("noise `{name}`: unknown dial `{dial}` (only `continent_size` scales noises)")));
            }
        }
        if !def.noises.iter().any(|(n, _)| n == "continents") {
            return Err(err("the noise `continents` is missing".into()));
        }
        for (name, def_expr) in &def.fields {
            if program.slot(name).is_some() {
                return Err(err(format!("`{name}` is defined twice")));
            }
            let root = program.expr(def_expr, &|what| err(format!("field `{name}`: {what}")))?;
            let climate = program.reads_climate(root);
            program.fields.push((name.clone(), root, climate));
        }
        let field = |name: &str| {
            program.fields.iter().position(|(n, _, _)| n == name).map(|i| Field(program.field_slot(i))).ok_or_else(|| err(format!("the field `{name}` is missing")))
        };
        program.outputs = Outputs {
            surface: field("surface")?,
            continental: field("C")?,
            mountain: field("mountain")?,
            peak: field("peak")?,
            roughness: field("roughness")?,
            valley: field("valley")?,
            crag: field("crag").ok(),
        };
        if program.depends_on_climate(program.outputs.mountain) || program.depends_on_climate(program.outputs.continental) {
            return Err(err("`C` and `mountain` must not read `T` or `H`: the climate's rain shadows read them".into()));
        }
        Ok(program)
    }

    fn field_slot(&self, index: usize) -> u16 {
        (INPUTS.len() + self.noises.len() + index) as u16
    }

    /// The slot a name refers to: a built-in input, a noise or a field defined so far.
    fn slot(&self, name: &str) -> Option<u16> {
        if let Some(i) = INPUTS.iter().position(|n| *n == name) {
            return Some(i as u16);
        }
        if let Some(i) = self.noises.iter().position(|(n, _)| n == name) {
            return Some((INPUTS.len() + i) as u16);
        }
        self.fields.iter().position(|(n, _, _)| n == name).map(|i| self.field_slot(i))
    }

    fn push(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        (self.nodes.len() - 1) as NodeId
    }

    fn expr(&mut self, def: &ExprDef, err: &dyn Fn(String) -> ContentError) -> Result<NodeId, ContentError> {
        let list = |program: &mut Self, items: &[ExprDef]| -> Result<(u32, u32), ContentError> {
            if items.is_empty() {
                return Err(err("an empty list".into()));
            }
            let ids = items.iter().map(|e| program.expr(e, err)).collect::<Result<Vec<_>, _>>()?;
            let first = program.args.len() as u32;
            program.args.extend(ids);
            Ok((first, items.len() as u32))
        };
        Ok(match def {
            ExprDef::Const(v) => self.push(Node::Const(*v)),
            ExprDef::Name(name) => match name.as_str() {
                "tectonic" | "young" => {
                    let id = self.push(Node::Const(0.0));
                    self.dials.push((id, if name == "tectonic" { Dial::Tectonic } else { Dial::Young }));
                    id
                }
                _ => {
                    let slot = self.slot(name).ok_or_else(|| err(format!("unknown name `{name}` (fields may only read the fields before them)")))?;
                    self.push(Node::Slot(slot))
                }
            },
            ExprDef::Op(op) => match op {
                OpDef::Spline { on, points } => {
                    if points.is_empty() {
                        return Err(err("a spline with no knots".into()));
                    }
                    let on = self.expr(on, err)?;
                    let mut knots = Vec::with_capacity(points.len());
                    for point in points {
                        let (at, value, slope) = point.parts();
                        let constant = match value {
                            ExprDef::Const(v) => Some(*v),
                            _ => None,
                        };
                        knots.push(Knot { at, value: self.expr(value, err)?, slope, constant });
                    }
                    if knots.windows(2).any(|w| w[1].at < w[0].at) {
                        return Err(err("spline knots out of order".into()));
                    }
                    if knots.len() > MAX_KNOTS {
                        return Err(err(format!("a spline with more than {MAX_KNOTS} knots")));
                    }
                    let first = self.knots.len() as u32;
                    let leaf = knots.iter().all(|k| k.constant.is_some());
                    self.knots.extend(knots);
                    self.push(Node::Spline { on, first, count: points.len() as u32, leaf })
                }
                OpDef::Add(items) => {
                    let (a, n) = list(self, items)?;
                    self.push(Node::Add(a, n))
                }
                OpDef::Mul(items) => {
                    let (a, n) = list(self, items)?;
                    self.push(Node::Mul(a, n))
                }
                OpDef::Min(items) => {
                    let (a, n) = list(self, items)?;
                    self.push(Node::Min(a, n))
                }
                OpDef::Max(items) => {
                    let (a, n) = list(self, items)?;
                    self.push(Node::Max(a, n))
                }
                OpDef::Abs(x) => {
                    let x = self.expr(x, err)?;
                    self.push(Node::Abs(x))
                }
                OpDef::Square(x) => {
                    let x = self.expr(x, err)?;
                    self.push(Node::Square(x))
                }
                OpDef::Clamp(x, lo, hi) => {
                    let x = self.expr(x, err)?;
                    self.push(Node::Clamp(x, *lo, *hi))
                }
                OpDef::Shifted(noise, dx, dz) => {
                    let noise = self.noises.iter().position(|(n, _)| n == noise).ok_or_else(|| err(format!("unknown noise `{noise}`")))?;
                    let (dx, dz) = (self.expr(dx, err)?, self.expr(dz, err)?);
                    self.push(Node::Shifted { noise: noise as u16, dx, dz })
                }
            },
        })
    }

    /// Whether a node reads `T` or `H`, directly or through a field.
    fn reads_climate(&self, node: NodeId) -> bool {
        let args = |first: u32, n: u32| self.args[first as usize..(first + n) as usize].iter().any(|&a| self.reads_climate(a));
        match self.nodes[node as usize] {
            Node::Const(_) => false,
            Node::Slot(slot) => self.depends_on_climate(Field(slot)),
            Node::Spline { on, first, count, .. } => {
                self.reads_climate(on) || self.knots[first as usize..(first + count) as usize].iter().any(|k| self.reads_climate(k.value))
            }
            Node::Add(a, n) | Node::Mul(a, n) | Node::Min(a, n) | Node::Max(a, n) => args(a, n),
            Node::Abs(x) | Node::Square(x) | Node::Clamp(x, _, _) => self.reads_climate(x),
            Node::Shifted { dx, dz, .. } => self.reads_climate(dx) || self.reads_climate(dz),
        }
    }

    fn depends_on_climate(&self, field: Field) -> bool {
        let slot = field.0 as usize;
        let first = INPUTS.len() + self.noises.len();
        match slot {
            s if s == T as usize || s == H as usize => true,
            s if s < first => false,
            s => self.fields[s - first].2,
        }
    }
}

/// The climate a land reads: `T` and `H` at up to four positions.
pub trait Inputs {
    fn climate(&self, x: [f32; 4], z: [f32; 4], n: usize) -> ([f32; 4], [f32; 4]);
}

/// For fields that read no climate, such as the ones rain shadows are made from.
pub struct NoClimate;

impl Inputs for NoClimate {
    fn climate(&self, _: [f32; 4], _: [f32; 4], _: usize) -> ([f32; 4], [f32; 4]) {
        unreachable!("a field that reads no climate asked for it")
    }
}

struct Noise {
    /// Each octave, at its own frequency, with its amplitude (normalisation included).
    octaves: Vec<(FastNoiseLite, f32)>,
    /// The same as (seed, frequency, amplitude), for `crate::noise::octaves_2d_x4`.
    kernel: Vec<(i32, f32, f32)>,
    warp: Option<usize>,
}

impl Noise {
    #[inline]
    fn at4(&self, x: [f32; 4], z: [f32; 4]) -> [f32; 4] {
        if let Some(sum) = crate::noise::octaves_2d_x4(&self.kernel, x, z) {
            return sum;
        }
        let mut sum = [0.0f32; 4];
        for (noise, amp) in &self.octaves {
            let v = crate::noise::get_2d_x4(noise, x, z);
            for k in 0..4 {
                sum[k] += amp * v[k];
            }
        }
        sum
    }

    /// One position at a time, the plain way, for checking `at4`.
    #[cfg(test)]
    fn at(&self, x: f32, z: f32) -> f32 {
        self.octaves.iter().map(|(noise, amp)| amp * noise.get_noise_2d(x, z)).sum()
    }
}

/// A land, seeded for one world.
pub struct Router {
    program: Program,
    nodes: Vec<Node>,
    /// Where each knot lies, side by side for finding segments.
    ats: Vec<f32>,
    noises: Vec<Noise>,
    warps: Vec<FastNoiseLite>,
    /// The `continents` noise's value at the coast, for the world's ocean fraction.
    coast: f32,
    continents: usize,
    pub outputs: Outputs,
}

/// Up to `LANES` positions being evaluated, and what is known about them so far.
pub struct Lanes {
    x: Wide,
    z: Wide,
    n: usize,
    values: Vec<Wide>,
    /// A bit per lane, for each slot, once its value is known.
    known: Vec<u8>,
    /// Warped positions, once known, by group of four lanes.
    warped: Vec<[Option<([f32; 4], [f32; 4])>; 2]>,
}

impl Lanes {
    /// A bit for each lane started.
    fn all(&self) -> u8 {
        ((1u16 << self.n) - 1) as u8
    }
}

/// Standard deviation of one octave of FastNoiseLite's 2D OpenSimplex2, measured once.
fn octave_spread() -> f32 {
    static SPREAD: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
        let mut n = FastNoiseLite::with_seed(0x51e7);
        n.set_noise_type(Some(NoiseType::OpenSimplex2));
        n.set_frequency(Some(1.0));
        let samples: Vec<f32> = (0..1 << 16).map(|i| n.get_noise_2d((i & 255) as f32 * 2.37, (i >> 8) as f32 * 2.37)).collect();
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        (samples.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / samples.len() as f32).sqrt()
    });
    *SPREAD
}

impl Router {
    pub fn new(seed: u64, dials: &WorldDials, program: &Program) -> Self {
        let seeded = |name: &str, salt: u64| (crate::rng::mix(seed ^ crate::rarity::name_hash(name) ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15)) >> 32) as i32;
        let warps = program
            .warps
            .iter()
            .map(|(name, def)| {
                let mut warp = FastNoiseLite::with_seed(seeded(name, 0));
                warp.set_noise_type(Some(NoiseType::OpenSimplex2));
                warp.set_frequency(Some(1.0 / def.wavelength));
                warp.set_fractal_type(Some(FractalType::DomainWarpProgressive));
                warp.set_fractal_octaves(Some(def.octaves));
                warp.set_domain_warp_type(Some(DomainWarpType::OpenSimplex2));
                warp.set_domain_warp_amp(Some(def.amplitude));
                warp
            })
            .collect();
        let noises: Vec<Noise> = program
            .noises
            .iter()
            .map(|(name, def)| {
                let scale = match def.scale_by.as_deref() {
                    Some("continent_size") => dials.continent_size / CONTINENT_SIZE,
                    _ => 1.0,
                };
                let weights: Vec<f32> = def.octaves.iter().enumerate().map(|(i, w)| w * 0.5f32.powi(i as i32)).collect();
                let norm = def.spread / (octave_spread() * weights.iter().map(|w| w * w).sum::<f32>().sqrt());
                let octaves: Vec<(FastNoiseLite, f32)> = weights
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| **w != 0.0)
                    .map(|(i, w)| {
                        let mut n = FastNoiseLite::with_seed(seeded(name, i as u64 + 1));
                        n.set_noise_type(Some(NoiseType::OpenSimplex2));
                        n.set_frequency(Some(2f32.powi(i as i32) / (def.wavelength * scale)));
                        (n, w * norm)
                    })
                    .collect();
                let kernel = octaves.iter().map(|(n, amp)| (n.seed, n.frequency, *amp)).collect();
                Noise { octaves, kernel, warp: def.warp.as_ref().map(|w| program.warps.iter().position(|(n, _)| n == w).expect("checked at load")) }
            })
            .collect();
        let mut nodes = program.nodes.clone();
        for &(node, dial) in &program.dials {
            nodes[node as usize] = Node::Const(match dial {
                Dial::Tectonic => dials.tectonic_activity,
                Dial::Young => 1.0 - dials.erosion_age,
            });
        }
        // Expressions of constants alone, such as those of the dials, become constants,
        // worked out as evaluation would. Nodes come after the nodes they read.
        for i in 0..nodes.len() {
            let constant = |n: &NodeId| match nodes[*n as usize] {
                Node::Const(v) => Some(v),
                _ => None,
            };
            let args = |a: u32, n: u32| program.args[a as usize..(a + n) as usize].iter().map(constant).collect::<Option<Vec<f32>>>();
            let folded = match nodes[i] {
                Node::Add(a, n) => args(a, n).map(|v| v.iter().fold(0.0, |sum, v| sum + v)),
                Node::Mul(a, n) => args(a, n).map(|v| v.iter().fold(1.0, |product, &v| if product == 0.0 { product } else { product * v })),
                Node::Min(a, n) => args(a, n).map(|v| v.iter().fold(f32::INFINITY, |m, &v| m.min(v))),
                Node::Max(a, n) => args(a, n).map(|v| v.iter().fold(f32::NEG_INFINITY, |m, &v| m.max(v))),
                Node::Abs(x) => constant(&x).map(f32::abs),
                Node::Square(x) => constant(&x).map(|v| v * v),
                Node::Clamp(x, lo, hi) => constant(&x).map(|v| v.clamp(lo, hi)),
                _ => None,
            };
            if let Some(v) = folded {
                nodes[i] = Node::Const(v);
            }
        }
        let continents = program.noises.iter().position(|(n, _)| n == "continents").expect("checked at load");
        // The sea takes `ocean_fraction` of the world where the noise's folded value is below
        // this: its quantile over a wide grid of the world's own noise, as a sum of a few
        // octaves is not quite normal.
        let coast = {
            let noise = &noises[continents];
            let step = 0.37 / noise.kernel.first().map_or(1.0, |octave| octave.1);
            let at = |i: usize| ((i % 64) as f32 * step, (i / 64) as f32 * step);
            let mut folded: Vec<f32> = (0..64 * 64)
                .step_by(4)
                .flat_map(|i| noise.at4(std::array::from_fn(|k| at(i + k).0), std::array::from_fn(|k| at(i + k).1)))
                .map(f32::abs)
                .collect();
            folded.sort_by(f32::total_cmp);
            folded[((folded.len() - 1) as f32 * dials.ocean_fraction) as usize]
        };
        let ats = program.knots.iter().map(|knot| knot.at).collect();
        Self { outputs: program.outputs, program: program.clone(), nodes, ats, noises, warps, coast, continents }
    }

    /// Fresh state for evaluating positions, to be reused across calls.
    pub fn lanes(&self) -> Lanes {
        let slots = INPUTS.len() + self.noises.len() + self.program.fields.len();
        Lanes { x: [0.0; LANES], z: [0.0; LANES], n: 0, values: vec![[0.0; LANES]; slots], known: vec![0; slots], warped: vec![[None; 2]; self.warps.len()] }
    }

    /// A field by name, for benchmarks.
    #[cfg(test)]
    pub fn field(&self, name: &str) -> Option<Field> {
        self.program.fields.iter().position(|(n, _, _)| n == name).map(|i| Field(self.program.field_slot(i)))
    }

    /// Starts evaluating one to `LANES` positions.
    pub fn start(&self, lanes: &mut Lanes, points: &[(f32, f32)]) {
        let n = points.len();
        debug_assert!((1..=LANES).contains(&n));
        lanes.n = n;
        lanes.x = std::array::from_fn(|k| points[k.min(n - 1)].0);
        lanes.z = std::array::from_fn(|k| points[k.min(n - 1)].1);
        lanes.known.fill(0);
        lanes.warped.fill([None; 2]);
    }

    /// The value of `field` at position `lane` of those started.
    #[inline]
    pub fn get(&self, lanes: &mut Lanes, inputs: &dyn Inputs, field: Field, lane: usize) -> f32 {
        if lanes.known[field.0 as usize] & (1 << lane) == 0 {
            let all = lanes.all();
            self.slot(lanes, inputs, field.0, all);
        }
        lanes.values[field.0 as usize][lane]
    }

    /// The ceiling heights above the sea ease under: (knee, room).
    pub fn ceiling(&self) -> (f32, f32) {
        self.program.ceiling
    }

    /// Height above the sea, in blocks, of land `surface` (in units of 128 blocks) high:
    /// scaled, stretched by its square and eased under the ceiling. The sea floor is left
    /// as it is.
    pub fn height(&self, surface: f32, scale: f32) -> f32 {
        let above = 128.0 * surface;
        if above <= 0.0 {
            return above;
        }
        let p = &self.program;
        let h = above * p.vertical_scale * scale;
        let h = h * (1.0 + p.elevation_boost * (h / 128.0).powi(2));
        ease(h, p.ceiling)
    }

    /// The values of a slot in the lanes of `mask`, working out those not known yet.
    fn slot(&self, lanes: &mut Lanes, inputs: &dyn Inputs, slot: u16, mask: u8) -> Wide {
        let s = slot as usize;
        let missing = mask & !lanes.known[s];
        if missing == 0 {
            return lanes.values[s];
        }
        let first_field = INPUTS.len() + self.noises.len();
        // The climate and noises are worked out a group of four lanes at a time, in every
        // lane of the groups asked for: four cost about as much as one.
        let whole = whole_groups(missing) & lanes.all();
        if slot == T || slot == H {
            for g in groups(missing) {
                let (x, z) = (quarter(&lanes.x, g), quarter(&lanes.z, g));
                let (t, h) = inputs.climate(x, z, (lanes.n - 4 * g).min(4));
                lanes.values[T as usize][4 * g..4 * g + 4].copy_from_slice(&t);
                lanes.values[H as usize][4 * g..4 * g + 4].copy_from_slice(&h);
            }
            lanes.known[T as usize] |= whole;
            lanes.known[H as usize] |= whole;
        } else if slot == FOLD {
            let continents = self.slot(lanes, inputs, (INPUTS.len() + self.continents) as u16, whole);
            for k in 0..LANES {
                if whole & (1 << k) != 0 {
                    lanes.values[s][k] = continents[k].abs() / self.coast;
                }
            }
            lanes.known[s] |= whole;
        } else if s < first_field {
            let noise = &self.noises[s - INPUTS.len()];
            for g in groups(missing) {
                let (x, z) = self.position(lanes, noise.warp, g);
                lanes.values[s][4 * g..4 * g + 4].copy_from_slice(&noise.at4(x, z));
            }
            lanes.known[s] |= whole;
        } else {
            let root = self.program.fields[s - first_field].1;
            let v = self.eval(lanes, inputs, root, missing);
            for (k, (value, v)) in lanes.values[s].iter_mut().zip(v).enumerate() {
                if missing & (1 << k) != 0 {
                    *value = v;
                }
            }
            lanes.known[s] |= missing;
        }
        lanes.values[s]
    }

    /// The positions of group `g` of four lanes, bent by a warp if any.
    fn position(&self, lanes: &mut Lanes, warp: Option<usize>, g: usize) -> ([f32; 4], [f32; 4]) {
        let (x, z) = (quarter(&lanes.x, g), quarter(&lanes.z, g));
        let Some(w) = warp else { return (x, z) };
        if let Some(p) = lanes.warped[w][g] {
            return p;
        }
        let p = crate::noise::warp_2d_x4(&self.warps[w], x, z);
        lanes.warped[w][g] = Some(p);
        p
    }

    /// The values of a node in the lanes of `mask` (the others mean nothing), all lanes
    /// worked out side by side.
    #[inline]
    fn eval(&self, lanes: &mut Lanes, inputs: &dyn Inputs, node: NodeId, mask: u8) -> Wide {
        // Most nodes name a field or noise, already known in the lanes asked for.
        match self.nodes[node as usize] {
            Node::Slot(slot) if mask & !lanes.known[slot as usize] == 0 => lanes.values[slot as usize],
            Node::Const(v) => [v; LANES],
            _ => self.eval_node(lanes, inputs, node, mask),
        }
    }

    fn eval_node(&self, lanes: &mut Lanes, inputs: &dyn Inputs, node: NodeId, mask: u8) -> Wide {
        let args = |first: u32, n: u32| &self.program.args[first as usize..(first + n) as usize];
        match self.nodes[node as usize] {
            Node::Const(v) => [v; LANES],
            Node::Slot(slot) => self.slot(lanes, inputs, slot, mask),
            Node::Spline { on, first, count, leaf } => {
                let x = self.eval(lanes, inputs, on, mask);
                let range = first as usize..(first + count) as usize;
                let (knots, segments) = (&self.program.knots[range.clone()], segments(&self.ats[range], x, mask));
                if leaf { leaf_spline(knots, x, mask, segments) } else { self.spline(lanes, inputs, knots, x, mask, segments) }
            }
            Node::Add(a, n) => {
                let mut sum = [0.0; LANES];
                for &arg in args(a, n) {
                    let v = self.eval(lanes, inputs, arg, mask);
                    for k in 0..LANES {
                        sum[k] += v[k];
                    }
                }
                sum
            }
            Node::Mul(a, n) => {
                // Lanes whose product is already zero need none of the factors after.
                let (mut product, mut live) = ([1.0; LANES], mask);
                for &arg in args(a, n) {
                    let v = self.eval(lanes, inputs, arg, live);
                    for k in 0..LANES {
                        if live & (1 << k) != 0 {
                            product[k] *= v[k];
                            if product[k] == 0.0 {
                                live &= !(1 << k);
                            }
                        }
                    }
                    if live == 0 {
                        break;
                    }
                }
                product
            }
            Node::Min(a, n) | Node::Max(a, n) => {
                let max = matches!(self.nodes[node as usize], Node::Max(..));
                let mut out = [if max { f32::NEG_INFINITY } else { f32::INFINITY }; LANES];
                for &arg in args(a, n) {
                    let v = self.eval(lanes, inputs, arg, mask);
                    for k in 0..LANES {
                        out[k] = if max { out[k].max(v[k]) } else { out[k].min(v[k]) };
                    }
                }
                out
            }
            Node::Abs(x) => self.eval(lanes, inputs, x, mask).map(f32::abs),
            Node::Square(x) => self.eval(lanes, inputs, x, mask).map(|v| v * v),
            Node::Clamp(x, lo, hi) => self.eval(lanes, inputs, x, mask).map(|v| v.clamp(lo, hi)),
            Node::Shifted { noise, dx, dz } => {
                let (dx, dz) = (self.eval(lanes, inputs, dx, mask), self.eval(lanes, inputs, dz, mask));
                let noise = &self.noises[noise as usize];
                let mut out = [0.0; LANES];
                for g in groups(mask) {
                    let (x, z) = self.position(lanes, noise.warp, g);
                    // Four lanes cost about as much as one; those outside the mask stay put, as
                    // their shifts mean nothing.
                    let shift = |p: [f32; 4], d: Wide| std::array::from_fn(|k| if mask & (1 << (4 * g + k)) != 0 { p[k] + d[4 * g + k] } else { p[k] });
                    out[4 * g..4 * g + 4].copy_from_slice(&noise.at4(shift(x, dx), shift(z, dz)));
                }
                out
            }
        }
    }

    /// A cubic Hermite spline through `knots` at `x` in the lanes of `mask`, as Minecraft
    /// evaluates its terrain splines: linear past the ends along their slopes. Each knot's
    /// value is worked out once, in the lanes that lie beside it; a lane exactly on a knot
    /// needs only that knot, as banded inputs such as the climate indices mostly are.
    fn spline(&self, lanes: &mut Lanes, inputs: &dyn Inputs, knots: &[Knot], x: Wide, mask: u8, (segment, common): ([usize; LANES], Option<usize>)) -> Wide {
        let value = |j: usize, lanes: &mut Lanes, mask: u8| match knots[j].constant {
            Some(v) => [v; LANES],
            None => self.eval(lanes, inputs, knots[j].value, mask),
        };
        let mut on_knot = 0u8;
        for k in 0..LANES {
            let i = segment[k];
            if mask & (1 << k) != 0 && i > 0 && knots[i - 1].at == x[k] {
                on_knot |= 1 << k;
            }
        }
        // Usually every lane lies in the same segment, as neighbouring positions do.
        if let Some(i) = common
            && mask != 0
        {
            let end = |j: usize, v: Wide| std::array::from_fn(|k| v[k] + knots[j].slope * (x[k] - knots[j].at));
            if i == 0 {
                return end(0, value(0, lanes, mask));
            }
            if i == knots.len() {
                return end(i - 1, value(i - 1, lanes, mask));
            }
            let y0 = value(i - 1, lanes, mask);
            let between = mask & !on_knot;
            if between == 0 {
                return y0;
            }
            let y1 = value(i, lanes, between);
            let (a, b) = (knots[i - 1], knots[i]);
            // Every lane on the curve side by side, then those on the knot take its value.
            let curve: Wide = std::array::from_fn(|k| hermite(a.at, y0[k], a.slope, b.at, y1[k], b.slope, x[k]));
            return std::array::from_fn(|k| if on_knot & (1 << k) != 0 { y0[k] } else { curve[k] });
        }
        // Otherwise the knots beside each lane: at most two per lane.
        let mut used: [(usize, u8); 2 * LANES] = [(0, 0); 2 * LANES];
        let mut count = 0;
        let mut need = |j: usize, k: usize| match used[..count].iter_mut().find(|u| u.0 == j) {
            Some(u) => u.1 |= 1 << k,
            None => {
                used[count] = (j, 1 << k);
                count += 1;
            }
        };
        for (k, &i) in segment.iter().enumerate() {
            if mask & (1 << k) != 0 {
                need(i.saturating_sub(1), k);
                if i > 0 && i < knots.len() && on_knot & (1 << k) == 0 {
                    need(i, k);
                }
            }
        }
        let mut values = [[0.0f32; LANES]; 2 * LANES];
        for (u, v) in used[..count].iter().zip(&mut values) {
            *v = value(u.0, lanes, u.1);
        }
        let value = |j: usize, k: usize| values[used[..count].iter().position(|u| u.0 == j).expect("needed above")][k];
        std::array::from_fn(|k| {
            if mask & (1 << k) == 0 {
                return 0.0;
            }
            let i = segment[k];
            if i == 0 || i == knots.len() {
                let j = i.saturating_sub(1);
                return value(j, k) + knots[j].slope * (x[k] - knots[j].at);
            }
            if on_knot & (1 << k) != 0 {
                return value(i - 1, k);
            }
            let (a, b) = (knots[i - 1], knots[i]);
            hermite(a.at, value(i - 1, k), a.slope, b.at, value(i, k), b.slope, x[k])
        })
    }
}

/// The segment of the knots at `ats` each lane lies in (the index of the first knot past
/// it), and the one every lane of `mask` lies in, if they share one. Counted rather than
/// searched for: splines have few knots, and a count has no branches to mispredict.
fn segments(ats: &[f32], x: Wide, mask: u8) -> ([usize; LANES], Option<usize>) {
    // Usually every lane lies in the first one's segment: counted for that lane alone, and
    // the others checked against the segment's ends. The knots are in order, so a lane lies
    // in it exactly when the knot before is at or below it and the knot after is not.
    let lead = x[(mask.trailing_zeros() as usize).min(LANES - 1)];
    let first = ats.iter().map(|&at| usize::from(at <= lead)).sum::<usize>();
    let inside = |x: f32| (first == 0 || ats[first - 1] <= x) && (first == ats.len() || !(ats[first] <= x));
    if (0..LANES).all(|k| mask & (1 << k) == 0 || inside(x[k])) {
        return ([first; LANES], Some(first));
    }
    let mut count = [0u32; LANES];
    for &at in ats {
        for k in 0..LANES {
            count[k] += u32::from(at <= x[k]);
        }
    }
    let first = count[(mask.trailing_zeros() as usize).min(LANES - 1)];
    let same = (0..LANES).all(|k| mask & (1 << k) == 0 || count[k] == first);
    (count.map(|c| c as usize), same.then_some(first as usize))
}

/// A spline whose knots are all numbers, in the lanes of `mask`: no bookkeeping of what to
/// work out, which most splines need not. When every lane lies in one segment, as usual,
/// all lanes are worked out side by side on the same curve.
fn leaf_spline(knots: &[Knot], x: Wide, mask: u8, (segment, common): ([usize; LANES], Option<usize>)) -> Wide {
    let value = |j: usize| knots[j].constant.unwrap_or_default();
    let end = |j: usize, x: f32| value(j) + knots[j].slope * (x - knots[j].at);
    if let Some(i) = common {
        if i == 0 || i == knots.len() {
            return x.map(|x| end(i.saturating_sub(1), x));
        }
        let (a, b, y0, y1) = (knots[i - 1], knots[i], value(i - 1), value(i));
        return x.map(|x| hermite(a.at, y0, a.slope, b.at, y1, b.slope, x));
    }
    std::array::from_fn(|k| {
        let i = segment[k];
        if mask & (1 << k) == 0 {
            0.0
        } else if i == 0 || i == knots.len() {
            end(i.saturating_sub(1), x[k])
        } else {
            let (a, b) = (knots[i - 1], knots[i]);
            hermite(a.at, value(i - 1), a.slope, b.at, value(i), b.slope, x[k])
        }
    })
}

/// The cubic Hermite segment from (x0, y0) with slope d0 to (x1, y1) with slope d1, at x.
#[inline]
fn hermite(x0: f32, y0: f32, d0: f32, x1: f32, y1: f32, d1: f32, x: f32) -> f32 {
    let span = x1 - x0;
    let t = (x - x0) / span;
    let p = d0 * span - (y1 - y0);
    let q = -d1 * span + (y1 - y0);
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    lerp(y0, y1) + t * (1.0 - t) * lerp(p, q)
}

/// Eases heights above the sea that pass `knee` towards `knee + room`.
fn ease(above: f32, (knee, room): (f32, f32)) -> f32 {
    if above > knee { knee + room * ((above - knee) / room).tanh() } else { above }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn land(source: &str) -> Result<Program, ContentError> {
        Program::compile(&ron::from_str(source).map_err(|e| ContentError(e.to_string()))?)
    }

    const MINIMAL: &str = r#"(
        noises: [("continents", (wavelength: 1000.0, octaves: [1.0, 1.0]))],
        fields: [
            ("C", "continents_fold"),
            ("mountain", 0.0), ("peak", 0.0), ("roughness", 0.0), ("valley", 0.0),
            ("surface", Spline(on: "C", points: [(0.0, -0.5), (1.0, 0.0, 0.2), (2.0, Mul([0.5, "tectonic"]))])),
        ],
        vertical_scale: 1.0, elevation_boost: 0.0, ceiling: (190.0, 48.0),
    )"#;

    #[test]
    fn hermite_matches_its_ends_and_slopes() {
        assert_eq!(hermite(0.0, 1.0, 0.0, 2.0, 3.0, 0.0, 0.0), 1.0);
        assert!((hermite(0.0, 1.0, 0.0, 2.0, 3.0, 0.0, 2.0) - 3.0).abs() < 1e-6);
        // Flat ends make an S between them, through the midpoint.
        assert!((hermite(0.0, 1.0, 0.0, 2.0, 3.0, 0.0, 1.0) - 2.0).abs() < 1e-6);
        let slope = (hermite(0.0, 0.0, 5.0, 1.0, 1.0, 0.0, 1e-3) - 0.0) / 1e-3;
        assert!((slope - 5.0).abs() < 0.05, "{slope}");
    }

    #[test]
    fn programs_are_checked() {
        land(MINIMAL).unwrap();
        let broken = MINIMAL.replace("(\"surface\"", "(\"surface\", \"nowhere\"), (\"other\"");
        assert!(land(&broken).is_err());
        let forward = MINIMAL.replace("(\"C\", \"continents_fold\")", "(\"C\", \"surface\")");
        assert!(land(&forward).unwrap_err().0.contains("unknown name"));
        let climate = MINIMAL.replace("(\"mountain\", 0.0)", "(\"mountain\", \"T\")");
        assert!(land(&climate).unwrap_err().0.contains("rain shadows"));
    }

    #[test]
    fn splines_evaluate_lazily_and_exactly() {
        let program = land(MINIMAL).unwrap();
        let dials = WorldDials::sample(7);
        let router = Router::new(7, &dials, &program);
        let mut lanes = router.lanes();
        let points = [(10.0, 20.0), (500.0, -300.0), (-2000.0, 77.0)];
        router.start(&mut lanes, &points);
        for (k, &(x, z)) in points.iter().enumerate() {
            let fold = router.get(&mut lanes, &NoClimate, router.outputs.continental, k);
            let surface = router.get(&mut lanes, &NoClimate, router.outputs.surface, k);
            // The same position alone gives the same values.
            let mut alone = router.lanes();
            router.start(&mut alone, &[(x, z)]);
            assert_eq!(router.get(&mut alone, &NoClimate, router.outputs.surface, 0), surface);
            let expected = if fold < 1.0 {
                hermite(0.0, -0.5, 0.0, 1.0, 0.0, 0.2, fold)
            } else if fold < 2.0 {
                hermite(1.0, 0.0, 0.2, 2.0, 0.5 * dials.tectonic_activity, 0.0, fold)
            } else {
                0.5 * dials.tectonic_activity
            };
            assert!((surface - expected).abs() < 1e-5, "{surface} {expected}");
        }
    }

    #[test]
    fn lanes_on_a_knot_need_only_that_knot() {
        let source = MINIMAL.replace(
            r#"("surface", Spline(on: "C", points: [(0.0, -0.5), (1.0, 0.0, 0.2), (2.0, Mul([0.5, "tectonic"]))])),"#,
            r#"("at", 2.0), ("far", Mul(["C", 3.0])), ("surface", Spline(on: "at", points: [(1.0, 0.0), (2.0, 0.5), (3.0, "far")])),"#,
        );
        let program = land(&source).unwrap();
        let router = Router::new(7, &WorldDials::sample(7), &program);
        let mut lanes = router.lanes();
        router.start(&mut lanes, &[(10.0, 20.0), (500.0, -300.0)]);
        assert_eq!(router.get(&mut lanes, &NoClimate, router.outputs.surface, 1), 0.5);
        assert_eq!(lanes.known[router.field("far").unwrap().0 as usize], 0, "the knot after was worked out");
    }

    /// The plain meaning of a node at one position, worked out the slow way: every field
    /// from scratch, every spline knot, no lanes, no caching, no shortcuts.
    fn reference(router: &Router, inputs: &dyn Inputs, node: NodeId, x: f32, z: f32) -> f32 {
        let eval = |n: NodeId| reference(router, inputs, n, x, z);
        let p = &router.program;
        let args = |a: u32, n: u32| p.args[a as usize..(a + n) as usize].iter().map(|&n| eval(n)).collect::<Vec<f32>>();
        match router.nodes[node as usize] {
            Node::Const(v) => v,
            Node::Slot(slot) => {
                let s = slot as usize;
                let first_field = INPUTS.len() + router.noises.len();
                if slot == T || slot == H {
                    let (t, h) = inputs.climate([x; 4], [z; 4], 1);
                    if slot == T { t[0] } else { h[0] }
                } else if slot == FOLD {
                    let noise = &router.noises[router.continents];
                    let (wx, wz) = warped(router, noise.warp, x, z);
                    noise.at(wx, wz).abs() / router.coast
                } else if s < first_field {
                    let noise = &router.noises[s - INPUTS.len()];
                    let (wx, wz) = warped(router, noise.warp, x, z);
                    noise.at(wx, wz)
                } else {
                    eval(p.fields[s - first_field].1)
                }
            }
            Node::Spline { on, first, count, .. } => {
                let knots = &p.knots[first as usize..(first + count) as usize];
                let x = eval(on);
                let i = knots.partition_point(|k| k.at <= x);
                if i == 0 || i == knots.len() {
                    let k = knots[i.saturating_sub(1)];
                    return eval(k.value) + k.slope * (x - k.at);
                }
                let (a, b) = (knots[i - 1], knots[i]);
                hermite(a.at, eval(a.value), a.slope, b.at, eval(b.value), b.slope, x)
            }
            Node::Add(a, n) => args(a, n).iter().sum(),
            Node::Mul(a, n) => args(a, n).iter().product(),
            Node::Min(a, n) => args(a, n).iter().copied().fold(f32::INFINITY, f32::min),
            Node::Max(a, n) => args(a, n).iter().copied().fold(f32::NEG_INFINITY, f32::max),
            Node::Abs(v) => eval(v).abs(),
            Node::Square(v) => eval(v) * eval(v),
            Node::Clamp(v, lo, hi) => eval(v).clamp(lo, hi),
            Node::Shifted { noise, dx, dz } => {
                let n = &router.noises[noise as usize];
                let (wx, wz) = warped(router, n.warp, x, z);
                n.at(wx + eval(dx), wz + eval(dz))
            }
        }
    }

    fn warped(router: &Router, warp: Option<usize>, x: f32, z: f32) -> (f32, f32) {
        match warp {
            Some(w) => {
                let (wx, wz) = crate::noise::warp_2d_x4(&router.warps[w], [x; 4], [z; 4]);
                (wx[0], wz[0])
            }
            None => (x, z),
        }
    }

    /// Climate inputs that vary smoothly, for tests.
    struct Smooth;

    impl Inputs for Smooth {
        fn climate(&self, x: [f32; 4], z: [f32; 4], _: usize) -> ([f32; 4], [f32; 4]) {
            (std::array::from_fn(|k| 15.0 + 12.0 * (x[k] / 3000.0).sin()), std::array::from_fn(|k| 1.0 + 0.8 * (z[k] / 2500.0).cos()))
        }
    }

    /// The base package's land, worked out `LANES` positions at a time with every shortcut,
    /// is exactly its plain meaning at each position.
    #[test]
    fn lanes_match_the_plain_meaning() {
        let registry = crate::test_registry();
        let content = crate::content::WorldContent::base(&registry).unwrap();
        let program = Program::compile(&content.land).unwrap();
        let router = Router::new(7, &WorldDials::sample(7), &program);
        let mut lanes = router.lanes();
        let mut rng = crate::rng::Rng::new(1, 2, 3, 4);
        // Rows of neighbours, which share their segments, and scattered points, which do not.
        let mut points: Vec<(f32, f32)> = (0..2000).map(|i| ((i % 50 * 8) as f32 + 3000.0, (i / 50 * 8) as f32 - 2000.0)).collect();
        points.extend((0..2000).map(|_| (rng.range(-30_000, 30_000) as f32, rng.range(-30_000, 30_000) as f32)));
        let fields: Vec<Field> = (0..program.fields.len()).map(|i| Field(program.field_slot(i))).collect();
        for some in points.chunks(LANES) {
            router.start(&mut lanes, some);
            for &field in &fields {
                for (k, &(x, z)) in some.iter().enumerate() {
                    let fast = router.get(&mut lanes, &Smooth, field, k);
                    let first_field = INPUTS.len() + router.noises.len();
                    let plain = reference(&router, &Smooth, program.fields[field.0 as usize - first_field].1, x, z);
                    assert!(fast == plain || (fast - plain).abs() <= 1e-5 * plain.abs().max(1.0), "{} at ({x}, {z}): {fast} against {plain}", program.fields[field.0 as usize - first_field].0);
                }
            }
        }
    }

    #[test]
    fn noises_have_the_spread_asked_for() {
        let program = land(&MINIMAL.replace("octaves: [1.0, 1.0]", "octaves: [1.0, 2.0, 1.0], spread: 0.4")).unwrap();
        let router = Router::new(3, &WorldDials::sample(3), &program);
        let noise = &router.noises[0];
        let values: Vec<f32> = (0..20_000).map(|i| noise.at((i % 141) as f32 * 333.0, (i / 141) as f32 * 333.0)).collect();
        let sd = (values.iter().map(|v| v * v).sum::<f32>() / values.len() as f32).sqrt();
        assert!((sd - 0.4).abs() < 0.04, "{sd}");
    }
}
