//! Terrain shapes: landforms that rise where the climate calls for them (Decision 7: biomes
//! shape terrain, done the way Tectonic and Terralith do it).
//!
//! Each shape definition (`terrain.ron`) names a kind (dunes, mesas, towers and so on) and
//! the boxes of the band table it rises in. Its weight at a position is a product of ramps
//! on the regional fields biomes are picked by (temperature and moisture at the regional
//! elevation, continentalness and the landform class), each fading in over a short ramp
//! just inside an edge of the biome table (`climate.ron`). So dunes rise where deserts lie,
//! with the desert's edges, yet the terrain never asks which biome won and the picker never
//! sees the shapes; moving a band edge moves biomes and shapes together.
//!
//! Rarer shapes take only some regions of their climate, chosen by a slow mask noise of
//! their own. Shapes that stand alone (volcanoes, arches) are seeded on a jittered grid
//! instead, as many as their rarity and the world's dials allow. Every shape is a pure
//! function of the seed and the position, and costs nothing where its weight is zero.
//!
//! Most shapes change the height of the land. Volcanoes, mesas, dunes, canyons, hills and
//! sea cliffs are terrain, which the climate cools with; towers and spires are features
//! standing on it, which it does not, so a karst tower in a jungle grows no snow cap.
//! Arches and leaning spires are solid in 3D and are added to the columns as they are built
//! (`sculpt`).

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::glam::{IVec2, Vec2};

use crate::climate::{self, Regional};
use crate::content::{Axis, ShapeKind, WorldContent};
use crate::dials::WorldDials;
use crate::geology::{COAST, Relief, noise, smootherstep, smoothstep};
use crate::rarity::{Rarity, Tier, name_hash};
use crate::rng::hash3;
use crate::rocks::{ProvinceMask, Rocks};
use crate::SEA_LEVEL;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId(pub u8);

/// How far inside a box's edge a shape fades in, per table axis: °C of temperature, a
/// share of the edge's moisture, continentalness, and landform class.
const RAMPS: [f32; 4] = [1.5, 0.15, 0.05, 0.06];
/// How far inside a matching rock province a shape fades in, in blocks.
const PROVINCE_RAMP: f32 = 250.0;
/// Spacing of the slow noise that picks the regions of a rarer shape, in blocks.
const REGION_SCALE: f32 = 1800.0;

/// What the shapes make of one position.
#[derive(Clone, Copy, Debug, Default)]
pub struct Shaped {
    /// Height the terrain shapes add, negative where they cut: the climate sees it.
    pub terrain: f32,
    /// Height of the features standing on the terrain, which the climate does not see.
    pub features: f32,
    /// The shape that changes the position most, if any does.
    pub shape: Option<ShapeId>,
    /// How much of that shape's full form rises here, 0 ..= 1.
    pub strength: f32,
}

/// The land a shape stands on.
pub struct Ground<'a> {
    /// The regional climate anywhere, for shapes that stand alone, which rise where the
    /// climate at their centre allows.
    pub regional: &'a dyn Fn(i32, i32) -> Regional,
    /// Height of the ground, hills included.
    pub height: f32,
    /// What the hills add to it, which some shapes smooth away.
    pub hills: f32,
    pub relief: &'a Relief,
}

/// A box of the band table, as a span of each table axis.
struct Gate {
    spans: [(f32, f32); 4],
}

impl Gate {
    fn weight(&self, fields: &[f32; 4]) -> f32 {
        let mut weight = 1.0;
        for (axis, (&(lo, hi), &v)) in self.spans.iter().zip(fields).enumerate() {
            let ramp = |edge: f32| if axis == 1 { RAMPS[1] * edge.abs().max(0.3) } else { RAMPS[axis] };
            if lo.is_finite() {
                weight *= smoothstep(lo, lo + ramp(lo), v);
            }
            if hi.is_finite() {
                weight *= 1.0 - smoothstep(hi - ramp(hi), hi, v);
            }
            if weight == 0.0 {
                break;
            }
        }
        weight
    }
}

struct Shape {
    kind: ShapeKind,
    /// Where it may rise; everywhere when empty.
    gates: Vec<Gate>,
    province: Option<ProvinceMask>,
    /// For regional rarity: the region noise, and the share of its range the shape takes.
    region: Option<(FastNoiseLite, f32)>,
    /// Whether it stands alone (volcanoes, lone mountains, arches) rather than covering
    /// its climate, and then the chance that a cell of its grid holds one.
    alone: bool,
    chance: f32,
    noises: [FastNoiseLite; 3],
    seed: u64,
}

pub struct Shapes {
    list: Vec<Shape>,
    names: Vec<String>,
    /// Direction the prevailing wind blows towards, which dunes face.
    wind: [f32; 2],
    /// The world's volcanism, which raises its volcanoes higher.
    volcanism: f32,
    /// Roughens the faces of 3D pieces.
    rough: FastNoiseLite,
}

/// A piece of rock a 3D shape adds to every column it reaches, anchored on the ground.
#[derive(Clone, Copy, Debug)]
pub enum Piece {
    /// A natural arch along `along`, from feet at `feet` heights `half` either side of
    /// `centre`, rising `height` over the line between them.
    Arch { centre: Vec2, along: Vec2, half: f32, feet: [f32; 2], height: f32, thick: f32, wide: f32 },
    /// A spire rising `height` from `base` (its ground, at `foot`), leaning over by `lean`
    /// at the top, more the higher it goes.
    Spire { base: Vec2, foot: f32, height: f32, radius: f32, lean: Vec2 },
}

/// The context 3D pieces are anchored in: the regional climate, the ground (rivers cut
/// in; not a number under water, where nothing is anchored), and how far a position lies
/// from the nearest biome border.
pub struct Anchors<'a> {
    pub regional: &'a dyn Fn(i32, i32) -> Regional,
    pub ground: &'a dyn Fn(i32, i32) -> f32,
    pub edge: &'a dyn Fn(i32, i32) -> f32,
}

impl Shapes {
    pub fn new(seed: u64, dials: &WorldDials, content: &WorldContent, rocks: &Rocks) -> Self {
        let named: Vec<(&str, Rarity)> = content.shapes.iter().map(|s| (s.name.as_str(), s.rarity)).collect();
        let boost = dials.featured.multipliers("shape", &named);
        let list = content
            .shapes
            .iter()
            .zip(boost)
            .map(|(def, boost)| {
                let seed = seed ^ name_hash(&def.name);
                let share = (def.rarity.multiplier() * boost).min(1.0);
                let alone = matches!(def.kind, ShapeKind::Volcanoes { .. } | ShapeKind::Mountain { .. } | ShapeKind::Arches { .. });
                let gates = def
                    .climate
                    .iter()
                    .map(|b| Gate {
                        spans: [Axis::Temperature, Axis::Moisture, Axis::Continentalness, Axis::Erosion].map(|axis| content.span(axis, b.bands(axis))),
                    })
                    .collect();
                Shape {
                    kind: def.kind,
                    gates,
                    province: def.province.as_deref().map(|p| rocks.mask(p)),
                    region: (!alone && share < 1.0).then(|| (noise(seed, 0x2e61, 1.0 / REGION_SCALE, FractalType::FBm, 2), share)),
                    alone,
                    chance: if alone { standalone_chance(def.kind, share, dials) } else { 1.0 },
                    noises: kind_noises(def.kind, seed),
                    seed,
                }
            })
            .collect();
        Self {
            list,
            names: content.shapes.iter().map(|s| s.name.clone()).collect(),
            wind: dials.wind,
            volcanism: dials.volcanism,
            rough: noise(seed, 0x2a6e, 1.0 / 6.0, FractalType::FBm, 2),
        }
    }

    /// The name of a shape (`base:dunes`).
    pub fn name(&self, id: ShapeId) -> &str {
        &self.names[id.0 as usize]
    }

    /// Whether a shape is a sea cliff, whose foot the waves notch.
    pub fn sea_cliff(&self, id: ShapeId) -> bool {
        matches!(self.list[id.0 as usize].kind, ShapeKind::SeaCliffs { .. })
    }

    /// The arches and leaning spires that reach the columns of blocks from `min` to `max`.
    /// A piece is found from its own anchor alone, so every column it reaches finds it the
    /// same.
    pub fn pieces(&self, min: IVec2, max: IVec2, anchors: &Anchors) -> Vec<Piece> {
        let mut pieces = Vec::new();
        let fields = |r: &Regional| [r.temperature(), r.moisture(), r.continentalness, r.erosion];
        for shape in &self.list {
            match shape.kind {
                ShapeKind::Arches { span, height } => {
                    let cell = span * 7.0;
                    let reach = span;
                    let first = ((min.as_vec2() - reach) / cell).floor().as_ivec2();
                    let last = ((max.as_vec2() + reach) / cell).floor().as_ivec2();
                    for cz in first.y..=last.y {
                        for cx in first.x..=last.x {
                            let (a, b) = (hash3(shape.seed, cx, 7, cz), hash3(shape.seed ^ 0xa4c, cx, 7, cz));
                            let unit = |k: usize| (([a, b][k / 4] >> (k % 4 * 16)) & 0xffff) as f32 / 65536.0;
                            if unit(0) >= shape.chance {
                                continue;
                            }
                            let centre = Vec2::new(cx as f32 + 0.25 + 0.5 * unit(1), cz as f32 + 0.25 + 0.5 * unit(2)) * cell;
                            let (x, z) = (centre.x as i32, centre.y as i32);
                            let regional = (anchors.regional)(x, z);
                            let weight = shape.weight(centre.x, centre.y, &fields(&regional), &regional);
                            // Inside the climate and well inside a biome, so it never straddles
                            // a border.
                            if weight < 0.6 || (anchors.edge)(x, z) < 24.0 {
                                continue;
                            }
                            let along = Vec2::from_angle(unit(3) * std::f32::consts::TAU);
                            let half = span * (0.35 + 0.45 * unit(4));
                            let ends = [centre - along * half, centre + along * half];
                            let feet = ends.map(|e| (anchors.ground)(e.x as i32, e.y as i32));
                            let rise = height * (0.6 + 0.6 * unit(5)) * (0.7 + 0.3 * weight);
                            // On dry feet, and not buried under a hill between them (a river
                            // between them is fine: the arch bridges it).
                            if feet.iter().any(|f| f.is_nan()) || (anchors.ground)(x, z) > feet[0].max(feet[1]) + 0.4 * rise {
                                continue;
                            }
                            pieces.push(Piece::Arch { centre, along, half, feet, height: rise, thick: 3.0 + 3.0 * unit(6), wide: 2.0 + 2.0 * unit(7) });
                        }
                    }
                }
                ShapeKind::Spires { height, spacing, lean } => {
                    let reach = spacing * 0.3 + 0.8 * height;
                    let first = ((min.as_vec2() - reach) / spacing).floor().as_ivec2();
                    let last = ((max.as_vec2() + reach) / spacing).floor().as_ivec2();
                    let cluster = &shape.noises[1];
                    for cz in first.y..=last.y {
                        for cx in first.x..=last.x {
                            // The cells the heightfield leaves to leaning spires.
                            let (centre, u) = cell(shape.seed, cx, cz);
                            let base = Vec2::from(centre) * spacing;
                            if u[3] >= lean || u[0] > 0.3 + 0.5 * cluster.get_noise_2d(base.x / (spacing * 5.0), base.y / (spacing * 5.0)) {
                                continue;
                            }
                            let (x, z) = (base.x as i32, base.y as i32);
                            let regional = (anchors.regional)(x, z);
                            let weight = shape.weight(base.x, base.y, &fields(&regional), &regional);
                            if weight < 0.3 {
                                continue;
                            }
                            let foot = (anchors.ground)(x, z);
                            if foot.is_nan() {
                                continue;
                            }
                            let rise = height * (0.5 + 0.7 * u[2]) * weight;
                            let tilt = 0.25 + 0.35 * (u[3] / lean);
                            let lean = Vec2::from_angle(u[1] * 97.0) * tilt * rise;
                            let radius = spacing * (0.09 + 0.08 * u[1]) + 1.0;
                            pieces.push(Piece::Spire { base, foot, height: rise, radius, lean });
                        }
                    }
                }
                _ => {}
            }
        }
        pieces
    }

    /// Whether a piece is solid at the block (x, y, z).
    pub fn solid(&self, piece: &Piece, x: i32, y: i32, z: i32) -> bool {
        let p = Vec2::new(x as f32 + 0.5, z as f32 + 0.5);
        let rough = || self.rough.get_noise_3d(x as f32, y as f32, z as f32);
        let y = y as f32;
        match *piece {
            Piece::Arch { centre, along, half, feet, height, thick, wide } => {
                let rel = p - centre;
                let u = rel.dot(along) / half;
                if u.abs() > 1.15 {
                    return false;
                }
                let across = rel.perp_dot(along).abs();
                let ground = feet[0] + (feet[1] - feet[0]) * ((u + 1.0) / 2.0).clamp(0.0, 1.0);
                let u4 = u.powi(4).min(1.0);
                if across > wide * (1.0 + 0.8 * u4) + 1.5 {
                    return false;
                }
                let n = rough();
                if across > wide * (1.0 + 0.8 * u4) * (1.0 + 0.25 * n) {
                    return false;
                }
                // An elliptic ribbon, thicker towards its feet, which stand on legs.
                let crown = ground + height * (1.0 - u.min(1.0).powi(2)).max(0.0).sqrt();
                let top = crown + 0.5 + n;
                let bottom = if u.abs() > 0.75 { ground - 4.0 } else { crown - thick * (1.0 + 1.5 * u4) * (1.0 + 0.2 * n) };
                (bottom..=top).contains(&y)
            }
            Piece::Spire { base, foot, height, radius, lean } => {
                let t = (y - foot + 3.0) / (height + 3.0);
                if !(0.0..=1.0).contains(&t) {
                    return false;
                }
                let axis = base + lean * t.powf(1.7);
                let r = radius * (1.0 - t).powf(0.8) + 0.4;
                let d = p.distance(axis);
                d < r + 1.5 && d < r * (1.0 + 0.15 * rough())
            }
        }
    }

    /// What the shapes make of (x, z), on ground of the given height whose regional climate
    /// is `regional`.
    pub fn shape(&self, x: i32, z: i32, ground: &Ground, regional: &Regional) -> Shaped {
        let fields = [regional.temperature(), regional.moisture(), regional.continentalness, regional.erosion];
        let (fx, fz) = (x as f32, z as f32);
        let mut out = Shaped::default();
        let mut most = 0.5;
        for (i, shape) in self.list.iter().enumerate() {
            if !shape.reaches(ground) {
                continue;
            }
            // Shapes that stand alone weigh their climate at their centre instead.
            let weight = if shape.alone { 1.0 } else { shape.weight(fx, fz, &fields, regional) };
            if weight <= 0.0 {
                continue;
            }
            let (terrain, features, strength) = self.rise(shape, fx, fz, weight, ground);
            out.terrain += terrain;
            out.features += features;
            let change = terrain.abs() + features;
            if change > most {
                most = change;
                out.shape = Some(ShapeId(i as u8));
                out.strength = strength;
            }
        }
        out
    }

    /// What one shape makes of a position where its weight is `weight`: the height it adds
    /// to the terrain and as features, and how much of its full form rises there.
    fn rise(&self, shape: &Shape, x: f32, z: f32, weight: f32, ground: &Ground) -> (f32, f32, f32) {
        let [n0, n1, n2] = &shape.noises;
        match shape.kind {
            ShapeKind::Hills { height, spacing } => {
                // Folded noise: round tops, creased hollows between them.
                let n = n0.get_noise_2d(x / spacing, z / spacing);
                (weight * height * (2.0 * n.abs()).min(1.5), 0.0, weight)
            }
            ShapeKind::Dunes { height, spacing } => {
                let [wx, wz] = self.wind;
                let (along, across) = ((x * wx + z * wz) / spacing, (z * wx - x * wz) / (spacing * 2.6));
                let crest = |along: f32| 1.0 - n0.get_noise_2d(along, across).abs();
                // Sampled upwind as well, the lee side of each crest falls away steeply.
                let here = crest(along);
                let dune = (here + 0.6 * (here - crest(along - 0.35))).clamp(0.0, 1.3);
                // Fields of tall dunes with calm margins, and some flattened crests; none on
                // the flanks of ranges.
                let amount = n1.get_noise_2d(x / (spacing * 14.0), z / (spacing * 14.0));
                let size = (1.0 - (amount / 0.6).powi(2)).clamp(0.2, 1.0) * (1.0 - smoothstep(0.1, 0.35, ground.relief.mountain));
                let cap = 0.8 + 0.35 * n2.get_noise_2d(x / (spacing * 4.0), z / (spacing * 4.0));
                let h = dune.min(cap).max(0.0).powf(1.4);
                (weight * size * height * h, 0.0, weight * size)
            }
            ShapeKind::Mesas { height, spacing } => {
                // Tiers at three levels of a mesa noise, lower towards the valleys, each mesa
                // its own height; risers a few blocks wide.
                let m = n0.get_noise_2d(x / spacing, z / spacing) - 0.5 * ground.relief.valley;
                let vary = 0.7 + 0.6 * n1.get_noise_2d(x / (spacing * 0.7), z / (spacing * 0.7)).abs();
                let mut level = 0.0;
                for (threshold, share) in [(-0.14, 0.4), (0.06, 0.3), (0.24, 0.3)] {
                    level += share * smoothstep(threshold, threshold + 0.022, m);
                }
                let rise = weight * level * height * vary;
                // The tops are flat: hills are smoothed away under them.
                // Every tier shows the shape's banded ground, from its first riser up.
                (rise - ground.hills * weight * smoothstep(0.0, 0.3, level), 0.0, weight * (level * 4.0).min(1.0))
            }
            ShapeKind::Canyons { depth, width } => {
                // Distance from the canyon's line, in half widths: the noise's typical slope
                // where it crosses zero (measured, about 3 per unit) turns its value into blocks.
                let scale = width * 14.0;
                let t = n0.get_noise_2d(x / scale, z / scale).abs() / (3.0 / scale * width * 0.5);
                if t >= 1.0 {
                    return (0.0, 0.0, 0.0);
                }
                let room = (ground.height - SEA_LEVEL as f32 - 5.0).max(0.0);
                let deep = (depth * (0.7 + 0.6 * n1.get_noise_2d(x / 900.0, z / 900.0).abs())).min(room);
                // Three steps down to a flat floor.
                let s = ((1.0 - t) / 0.75).min(1.0) * 3.0;
                let stepped = (s.floor() + smoothstep(0.75, 1.0, s.fract())) / 3.0;
                let cut = weight * deep * stepped;
                (-cut, 0.0, cut / depth)
            }
            ShapeKind::Towers { height, spacing } => {
                // Towers in clusters, of every size, some merging into ridges, with ragged
                // outlines; smaller knolls between them.
                let cluster = n1.get_noise_2d(x / (spacing * 6.0), z / (spacing * 6.0));
                let ragged = 1.0 + 0.1 * n0.get_noise_2d(x / 6.0, z / 6.0) + 0.2 * n2.get_noise_2d(x / 20.0, z / 20.0);
                let profile = |t: f32| {
                    let dome = (1.0 - 0.3 * t * t) * (1.0 - smoothstep(0.86, 1.0, t));
                    dome.max(0.12 * (1.0 - smoothstep(0.9, 1.35, t)))
                };
                let (big, _) = self.cells(shape, x, z, spacing, |u, t| {
                    if u[0] > 0.45 + 0.45 * cluster {
                        return 0.0;
                    }
                    (0.3 + 0.7 * u[2] * u[2].max(u[3])) * profile(t / (0.15 + 0.3 * u[1]) * ragged)
                });
                let (small, _) = self.cells(shape, x + 1e4, z, spacing * 0.45, |u, t| {
                    if u[0] > 0.35 + 0.3 * cluster {
                        return 0.0;
                    }
                    (0.12 + 0.25 * u[2]) * profile(t / (0.2 + 0.2 * u[1]) * ragged)
                });
                let tower = big.max(small);
                // The plain between the towers is flat.
                (-0.6 * ground.hills * weight, weight * height * tower, weight * tower)
            }
            ShapeKind::Spires { height, spacing, lean } => {
                let cluster = n1.get_noise_2d(x / (spacing * 5.0), z / (spacing * 5.0));
                let (spike, _) = self.cells(shape, x, z, spacing, |u, t| {
                    // Leaning spires are built in 3D; the rest come in clusters.
                    if u[3] < lean || u[0] > 0.3 + 0.5 * cluster {
                        return 0.0;
                    }
                    let t = t / (0.1 + 0.12 * u[1]);
                    let point = (1.0 - t).max(0.0).powf(0.8);
                    let skirt = 0.12 * (1.0 - smoothstep(1.0, 2.2, t));
                    (0.35 + 0.65 * u[2]) * point.max(skirt)
                });
                (0.0, weight * height * spike, weight * spike)
            }
            ShapeKind::Volcanoes { height, radius } => {
                // A clean cone: the hills under it are smoothed away towards its summit.
                let (rise, summit) = self.volcano(shape, x, z, height, radius, ground);
                (rise - ground.hills * smoothstep(0.0, 0.5, summit), 0.0, summit)
            }
            ShapeKind::Mountain { height, radius } => {
                let (rise, summit) = self.mountain(shape, x, z, height, radius, ground);
                (rise, 0.0, summit)
            }
            ShapeKind::SeaCliffs { height } => {
                // Upland held high right to the shore, then falling sheer to the sea.
                let c = ground.relief.continental;
                let rim = smoothstep(COAST - 0.002, COAST + 0.003, c) * (1.0 - smoothstep(COAST + 0.02, COAST + 0.12, c));
                let tall = weight * height * (0.6 + 0.4 * n0.get_noise_2d(x / 300.0, z / 300.0));
                // Stacks left standing offshore as the cliffs wore back.
                let offshore = smoothstep(COAST - 0.035, COAST - 0.022, c) * (1.0 - smoothstep(COAST - 0.007, COAST - 0.002, c));
                let mut stack = 0.0;
                if offshore > 0.0 {
                    let (pillar, u) = self.cells(shape, x, z, 34.0, |u, t| {
                        let t = t / (0.1 + 0.14 * u[1]) * (1.0 + 0.12 * n1.get_noise_2d(x / 5.0, z / 5.0));
                        if u[0] < 0.4 { 1.0 - smoothstep(0.8, 1.0, t) } else { 0.0 }
                    });
                    let top = SEA_LEVEL as f32 + tall * (0.35 + 0.6 * u[2]) * offshore;
                    stack = pillar * (top - ground.height).max(0.0);
                }
                (tall * rim + stack, 0.0, weight * rim.max(offshore * (stack > 0.0) as u8 as f32))
            }
            ShapeKind::Fjords { depth } => {
                // The floor of a broad valley sinks below the sea where it meets the coast and
                // climbs back out of it inland, between steep walls.
                let inland = smoothstep(COAST, COAST + 0.45, ground.relief.continental);
                let floor = SEA_LEVEL as f32 - depth * (1.0 - inland) + 12.0 * inland;
                let walls = smoothstep(0.55, 0.95, ground.relief.valley) * (1.0 - smoothstep(0.85, 1.0, inland));
                let cut = weight * walls * (ground.height - floor).max(0.0);
                (-cut, 0.0, weight * walls)
            }
            ShapeKind::Arches { .. } => (0.0, 0.0, 0.0),
        }
    }

    /// The largest of `f` over the cells of a jittered grid around (x, z), `spacing`
    /// apart, and the uniform values of the cell that gave it. `f` is given four uniform
    /// values for the cell and the distance from its centre in spacings.
    fn cells(&self, shape: &Shape, x: f32, z: f32, spacing: f32, f: impl Fn([f32; 4], f32) -> f32) -> (f32, [f32; 4]) {
        let (px, pz) = (x / spacing, z / spacing);
        let (cx, cz) = (px.floor() as i32, pz.floor() as i32);
        let mut best = (0.0f32, [0.0; 4]);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (centre, units) = cell(shape.seed, cx + dx, cz + dz);
                let t = (centre[0] - px).hypot(centre[1] - pz);
                if t < 1.0 {
                    let v = f(units, t);
                    if v > best.0 {
                        best = (v, units);
                    }
                }
            }
        }
        best
    }

    /// The standalone shape of the grid cell holding (x, z), if the cell holds one and it
    /// may reach `reach` blocks from its centre to the position: each sits well inside its
    /// cell, so no other cell can reach the position. Gives its centre, eight uniform values
    /// of its own, and its weight in the climate at its centre.
    fn standalone(&self, shape: &Shape, x: f32, z: f32, cell: f32, reach: f32, ground: &Ground) -> Option<Standalone> {
        let (cx, cz) = ((x / cell).floor() as i32, (z / cell).floor() as i32);
        let a = hash3(shape.seed, cx, 0, cz);
        if (a & 0xffff) as f32 / 65536.0 >= shape.chance {
            return None;
        }
        let b = hash3(shape.seed ^ 0xa1, cx, 0, cz);
        let unit = |k: usize| (([a, b][k / 4] >> (k % 4 * 16)) & 0xffff) as f32 / 65536.0;
        let units = std::array::from_fn(|k| unit(k));
        let centre = [(cx as f32 + 0.3 + 0.4 * unit(1)) * cell, (cz as f32 + 0.3 + 0.4 * unit(2)) * cell];
        // Out of its reach, the climate at its centre need not be weighed.
        if (x - centre[0]).hypot(z - centre[1]) >= reach {
            return None;
        }
        let regional = (ground.regional)(centre[0] as i32, centre[1] as i32);
        let fields = [regional.temperature(), regional.moisture(), regional.continentalness, regional.erosion];
        let weight = shape.weight(centre[0], centre[1], &fields, &regional);
        (weight > 0.2).then_some(Standalone { centre, cell: [cx, cz], units, weight })
    }

    /// A volcano's cone at (x, z), if one stands there: the height it adds and how near its
    /// summit the position is, 0 ..= 1.
    fn volcano(&self, shape: &Shape, x: f32, z: f32, height: f32, radius: f32, ground: &Ground) -> (f32, f32) {
        let Some(cone) = self.standalone(shape, x, z, radius * 6.0, radius * 1.25 * 1.22, ground) else { return (0.0, 0.0) };
        let height = height * cone.weight * (0.75 + 0.4 * self.volcanism);
        let u = cone.units;
        let r = radius * (0.75 + 0.5 * u[3]);
        let Some((t, cos, sin, d)) = cone.reach(shape, x, z, r, 0.22) else { return (0.0, 0.0) };
        // Concave flanks that meet the land without a crease, and a crater with a rim.
        let flank = |t: f32| (1.0 - t).powf(1.7) * smootherstep(0.0, 0.35, 1.0 - t);
        let crater = 0.14 + 0.12 * u[4];
        let rim = flank(crater);
        let floor = rim - 0.12 - 0.2 * u[5];
        let mut h = if t >= crater { flank(t) } else { floor + (rim - floor) * smoothstep(crater * 0.6, crater, t) };
        // Gullies down the flanks.
        let k = r / 22.0;
        let gully = shape.noises[1].get_noise_3d(cos * k, sin * k, d / 40.0).abs();
        h *= 1.0 - 0.08 * (1.0 - gully) * smoothstep(crater, crater + 0.1, t);
        (height * (0.55 + 0.45 * u[6]) * h, 1.0 - t)
    }

    /// A lone mountain at (x, z), if one stands there: a massif rising alone from the land
    /// around it, with spurs radiating from its summit.
    fn mountain(&self, shape: &Shape, x: f32, z: f32, height: f32, radius: f32, ground: &Ground) -> (f32, f32) {
        let Some(peak) = self.standalone(shape, x, z, radius * 6.0, radius * 1.2 * 1.3, ground) else { return (0.0, 0.0) };
        let height = height * peak.weight;
        let u = peak.units;
        let r = radius * (0.8 + 0.4 * u[3]);
        let Some((t, cos, sin, d)) = peak.reach(shape, x, z, r, 0.3) else { return (0.0, 0.0) };
        // Spurs and the gullies between them run down from the summit.
        let k = r / 70.0;
        let spur = 1.0 - shape.noises[1].get_noise_3d(cos * k, sin * k, d / 90.0).abs();
        let crags = shape.noises[2].get_noise_2d(x / 45.0, z / 45.0) * 0.5 + 0.5;
        let body = (1.0 - t).powf(1.25) * smootherstep(0.0, 0.3, 1.0 - t);
        let h = body * (0.82 + 0.18 * spur * smoothstep(0.05, 0.3, t)) + 0.04 * crags * (1.0 - t);
        (height * (0.8 + 0.2 * u[5]) * h, 1.0 - t)
    }
}

/// A standalone shape of a grid cell.
struct Standalone {
    centre: [f32; 2],
    cell: [i32; 2],
    units: [f32; 8],
    /// How strongly the climate at its centre lets it rise, 0 ..= 1.
    weight: f32,
}

impl Standalone {
    /// Where (x, z) lies on a round shape of radius `r` whose outline wanders by up to
    /// `wander` of it with the angle: its distance from the centre over the outline's, the
    /// direction from the centre, and the distance. `None` outside it.
    fn reach(&self, shape: &Shape, x: f32, z: f32, r: f32, wander: f32) -> Option<(f32, f32, f32, f32)> {
        let (dx, dz) = (x - self.centre[0], z - self.centre[1]);
        let d = dx.hypot(dz);
        if d >= r * (1.0 + wander) {
            return None;
        }
        // Noise sampled around a circle, so it wanders with the angle and closes up.
        let (cos, sin) = if d > 0.0 { (dx / d, dz / d) } else { (1.0, 0.0) };
        let (ox, oz) = (self.cell[0] as f32 * 17.0, self.cell[1] as f32 * 17.0);
        let outline = 1.0 + wander * shape.noises[0].get_noise_2d(ox + 1.4 * cos, oz + 1.4 * sin);
        let t = d / (r * outline);
        (t < 1.0).then_some((t, cos, sin, d))
    }
}

impl Shape {
    /// Whether the shape can change ground like this at all, whatever its climate: a cheap
    /// test that spares weighing it.
    fn reaches(&self, ground: &Ground) -> bool {
        let c = ground.relief.continental;
        match self.kind {
            ShapeKind::SeaCliffs { .. } => c > COAST - 0.035 && c < COAST + 0.12,
            ShapeKind::Fjords { .. } => ground.relief.valley > 0.55,
            // Built in 3D, by `pieces`.
            ShapeKind::Arches { .. } => false,
            _ => true,
        }
    }

    /// How strongly the shape rises at a position, 0 ..= 1, from its climate gates, rock
    /// province and rarity.
    fn weight(&self, x: f32, z: f32, fields: &[f32; 4], regional: &Regional) -> f32 {
        let mut weight = if self.gates.is_empty() { 1.0 } else { self.gates.iter().map(|g| g.weight(fields)).fold(0.0, f32::max) };
        if weight > 0.0
            && let Some(province) = &self.province
        {
            weight *= if province.contains(x, z, regional.warp) { smoothstep(0.0, PROVINCE_RAMP, province.depth(x, z, regional.warp)) } else { 0.0 };
        }
        if weight > 0.0
            && let Some((noise, share)) = &self.region
        {
            let u = climate::uniform(noise.get_noise_2d(x, z));
            let fade = (share / 2.0).min(0.03);
            weight *= smoothstep(1.0 - share, 1.0 - share + fade, u);
        }
        weight
    }
}

/// The centre of a cell of a jittered grid, in cells, and four uniform values for it.
fn cell(seed: u64, cx: i32, cz: i32) -> ([f32; 2], [f32; 4]) {
    let h = hash3(seed, cx, 0, cz);
    let jitter = |k: u32| 0.2 + 0.6 * ((h >> (k * 16)) & 0xffff) as f32 / 65536.0;
    let h2 = hash3(seed ^ 0x5eed, cx, 0, cz);
    let units = std::array::from_fn(|k| ((h2 >> (k * 16)) & 0xffff) as f32 / 65536.0);
    ([cx as f32 + jitter(0), cz as f32 + jitter(1)], units)
}

/// The noises each kind reads, at frequency 1 (callers scale positions themselves).
fn kind_noises(kind: ShapeKind, seed: u64) -> [FastNoiseLite; 3] {
    let octaves = match kind {
        ShapeKind::Hills { .. } | ShapeKind::Mesas { .. } | ShapeKind::Canyons { .. } => 2,
        _ => 1,
    };
    [noise(seed, 0x5a7e, 1.0, FractalType::FBm, octaves), noise(seed, 0x5a7f, 1.0, FractalType::FBm, 2), noise(seed, 0x5a80, 1.0, FractalType::FBm, 1)]
}

/// The chance that a cell of a standalone shape's grid holds one: about one volcano per
/// fifteen square kilometres in a moderately volcanic world, at rare; one lone mountain
/// per few hundred square kilometres, at legendary.
fn standalone_chance(kind: ShapeKind, share: f32, dials: &WorldDials) -> f32 {
    let base = match kind {
        ShapeKind::Volcanoes { .. } => 0.15 * (0.15 + 1.4 * dials.volcanism),
        // Lone mountains rise where the crust is restless, a few per thousand km² at most.
        ShapeKind::Mountain { .. } => 0.4 * (0.5 + dials.tectonic_activity),
        // Arches: one in several square kilometres of dry country, at very rare.
        ShapeKind::Arches { .. } => 0.06,
        _ => 1.0,
    };
    (base * share / Rarity::Rare.multiplier()).min(1.0)
}

impl Piece {
    /// The heights the piece spans.
    pub fn heights(&self) -> (i32, i32) {
        match *self {
            Piece::Arch { feet, height, .. } => (feet[0].min(feet[1]) as i32 - 5, (feet[0].max(feet[1]) + height) as i32 + 3),
            Piece::Spire { foot, height, .. } => (foot as i32 - 4, (foot + height) as i32 + 1),
        }
    }

    /// The square of columns the piece may reach between heights `low` and `high`, if it
    /// reaches that high: a cheap bound that lets whole sections of a column skip it.
    pub fn extent(&self, low: i32, high: i32) -> Option<(IVec2, IVec2)> {
        let (bottom, top) = self.heights();
        if high < bottom || low > top {
            return None;
        }
        let (centre, radius) = match *self {
            Piece::Arch { centre, half, wide, .. } => (centre, half * 1.15 + wide * 2.5 + 2.0),
            Piece::Spire { base, foot, height, radius, lean } => {
                // The axis between the two heights, and the widest the spire is there.
                let t = |y: i32| ((y as f32 - foot + 3.0) / (height + 3.0)).clamp(0.0, 1.0);
                let (a, b) = (base + lean * t(low).powf(1.7), base + lean * t(high).powf(1.7));
                ((a + b) / 2.0, a.distance(b) / 2.0 + radius * 1.15 + 2.0)
            }
        };
        Some(((centre - radius).floor().as_ivec2(), (centre + radius).ceil().as_ivec2()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_ramp_in_inside_their_edges() {
        // Temperature 17 °C and up, moisture below 0.34.
        let gate = Gate { spans: [(17.0, f32::INFINITY), (f32::NEG_INFINITY, 0.34), (f32::NEG_INFINITY, f32::INFINITY), (0.0, 0.5)] };
        let at = |t: f32, m: f32| gate.weight(&[t, m, 0.3, 0.2]);
        assert_eq!(at(16.9, 0.1), 0.0);
        assert_eq!(at(25.0, 0.4), 0.0);
        assert_eq!(at(25.0, 0.1), 1.0);
        // Fading in just inside the edges, never past them.
        let (warm, dry) = (at(17.5, 0.1), at(25.0, 0.32));
        assert!(0.0 < warm && warm < 1.0 && 0.0 < dry && dry < 1.0, "{warm} {dry}");
        assert!(at(18.0, 0.1) > warm);
        assert_eq!(gate.weight(&[25.0, 0.1, 0.3, 0.5]), 0.0);
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// `cargo test --release -p voxmar_worldgen shape_timing -- --ignored --nocapture`
    /// prints the time each terrain shape takes per position, weighing and rising.
    #[test]
    #[ignore]
    fn shape_timing() {
        let registry = voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap();
        let world = crate::Overworld::new(7, std::sync::Arc::new(registry));
        let geology = &world.geology;
        let shapes = geology.shapes();
        let points: Vec<(i32, i32, Relief, Regional)> = (0..40_000)
            .map(|i| {
                let (x, z) = (i % 200 * 13 - 1300, i / 200 * 13 - 1300);
                (x, z, geology.relief(x as f32, z as f32), geology.regional(x, z))
            })
            .collect();
        let anywhere = |x: i32, z: i32| geology.regional(x, z);
        for (i, shape) in shapes.list.iter().enumerate() {
            let start = std::time::Instant::now();
            let mut sum = 0.0;
            let mut active = 0;
            for (x, z, relief, regional) in &points {
                let ground = Ground { regional: &anywhere, height: relief.elevation, hills: 0.0, relief };
                if !shape.reaches(&ground) {
                    continue;
                }
                let fields = [regional.temperature(), regional.moisture(), regional.continentalness, regional.erosion];
                let weight = if shape.alone { 1.0 } else { shape.weight(*x as f32, *z as f32, &fields, regional) };
                if weight > 0.0 {
                    active += 1;
                    let (t, f, _) = shapes.rise(shape, *x as f32, *z as f32, weight, &ground);
                    sum += t + f;
                }
            }
            std::hint::black_box(sum);
            println!("{:24} {:6.1} ns, active at {:.0}%", shapes.names[i], start.elapsed().as_nanos() as f64 / points.len() as f64, 100.0 * active as f64 / points.len() as f64);
        }
    }
}
