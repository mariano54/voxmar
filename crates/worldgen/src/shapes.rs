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
//! their own. Every outline follows noise: towers, spires and sea stacks rise where a noise
//! passes a threshold, so no two are alike and none is round; only volcanoes, which really
//! are cones around a vent, are seeded on a jittered grid, gathered into volcanic fields
//! (regions of a mask like the rarer shapes') as many as their rarity and the world's dials
//! allow, on land, warped by a third of their radius so their outlines are irregular, and
//! their craters breached. Every shape is a pure function of the seed and the position, and
//! costs nothing where its weight is zero.
//!
//! Most shapes change the height of the land. Volcanoes, mesas, dunes, canyons, hills and
//! sea cliffs are terrain, which the climate cools with; towers, spires and the fins of
//! arches are features standing on it, which it does not, so a karst tower in a jungle grows
//! no snow cap. An arch is a fin with a window under it (`Shaped::window`), which
//! generation opens in the column.

use fastnoise_lite::{FastNoiseLite, FractalType};

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
/// How far a volcano's outline wanders from a circle, as a share of its radius.
const WANDER: f32 = 0.35;
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
    /// How high a window opens under the features from their foot, in whole blocks: the
    /// arch of a fin of rock.
    pub window: f32,
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
    /// Whether it stands alone (volcanoes) rather than covering its climate, and then the
    /// chance that a cell of its grid holds one.
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
                let alone = matches!(def.kind, ShapeKind::Volcanoes { .. });
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
                    // Volcanoes keep to volcanic fields, regions of the same mask; within one,
                    // most cells hold a cone.
                    region: if alone {
                        Some((noise(seed, 0x2e61, 1.0 / REGION_SCALE, FractalType::FBm, 2), standalone_chance(def.kind, share, dials).1))
                    } else {
                        (share < 1.0).then(|| (noise(seed, 0x2e61, 1.0 / REGION_SCALE, FractalType::FBm, 2), share))
                    },
                    alone,
                    chance: if alone { standalone_chance(def.kind, share, dials).0 } else { 1.0 },
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

    /// What the shapes make of (x, z), on ground of the given height whose regional climate
    /// is `regional`. The land asks through `shape_lanes`; this is for tests and benchmarks.
    #[cfg(test)]
    pub fn shape(&self, x: i32, z: i32, ground: &Ground, regional: &Regional) -> Shaped {
        self.shape_lanes(&[(x, z)], std::slice::from_ref(ground), std::slice::from_ref(regional))[0]
    }

    /// What the shapes make of one to four positions, each on its ground and in its
    /// regional climate, exactly as `shape` at each. The noises most positions sample
    /// (the masks that keep rarer shapes to some regions, and the rolling hills) are worked
    /// out side by side, one position per lane (see `crate::noise`); lanes past the
    /// positions mean nothing.
    pub fn shape_lanes(&self, at: &[(i32, i32)], grounds: &[Ground], regionals: &[Regional]) -> [Shaped; 4] {
        let n = at.len();
        let x: [f32; 4] = std::array::from_fn(|k| at[k.min(n - 1)].0 as f32);
        let z: [f32; 4] = std::array::from_fn(|k| at[k.min(n - 1)].1 as f32);
        // A noise at the lanes that need it: alone if one does, side by side if more.
        let sample = |noise: &FastNoiseLite, need: &[bool; 4], x: [f32; 4], z: [f32; 4]| -> [f32; 4] {
            let mut lanes = (0..n).filter(|&k| need[k]);
            match (lanes.next(), lanes.next()) {
                (None, _) => [0.0; 4],
                (Some(k), None) => {
                    let mut v = [0.0; 4];
                    v[k] = noise.get_noise_2d(x[k], z[k]);
                    v
                }
                _ => crate::noise::get_2d_x4(noise, x, z),
            }
        };
        let fields: [[f32; 4]; 4] = std::array::from_fn(|k| {
            let Some(r) = regionals.get(k) else { return [0.0; 4] };
            [r.temperature(), r.moisture(), r.continentalness, r.erosion]
        });
        let mut out = [Shaped::default(); 4];
        let mut most = [0.5f32; 4];
        for (i, shape) in self.list.iter().enumerate() {
            let mut weight = [0.0f32; 4];
            for k in 0..n {
                if shape.reaches(&grounds[k]) {
                    // Shapes that stand alone weigh their climate at their centre instead.
                    weight[k] = if shape.alone { 1.0 } else { shape.climate_weight(x[k], z[k], &fields[k], &regionals[k]) };
                }
            }
            // Risen only where its weight is not zero (a NaN weight would rise, as it always
            // has).
            let active = weight.map(|w| w > 0.0 || w.is_nan());
            // Shapes that stand alone weigh their regions at their centre.
            if let Some((noise, share)) = shape.region.as_ref().filter(|_| !shape.alone) {
                let need = active;
                let mask = sample(noise, &need, x, z);
                for k in (0..n).filter(|&k| need[k]) {
                    weight[k] *= shape.region_share(*share, mask[k]);
                }
            }
            let active = weight.map(|w| w > 0.0 || w.is_nan());
            let rise: [(f32, f32, f32); 4] = match shape.kind {
                ShapeKind::Hills { height, spacing } => {
                    let folded = sample(&shape.noises[0], &active, x.map(|x| x / spacing), z.map(|z| z / spacing));
                    std::array::from_fn(|k| if active[k] { hills(weight[k], height, folded[k]) } else { (0.0, 0.0, 0.0) })
                }
                _ => std::array::from_fn(|k| if k < n && active[k] { self.rise(shape, x[k], z[k], weight[k], &grounds[k]) } else { (0.0, 0.0, 0.0) }),
            };
            for k in (0..n).filter(|&k| active[k]) {
                let (terrain, features, strength) = rise[k];
                out[k].terrain += terrain;
                out[k].features += features;
                if features > 0.0 && matches!(shape.kind, ShapeKind::Arches { .. }) {
                    out[k].window = out[k].window.max(self.window(shape, x[k], z[k], features));
                }
                let change = terrain.abs() + features;
                if change > most[k] {
                    most[k] = change;
                    out[k].shape = Some(ShapeId(i as u8));
                    out[k].strength = strength;
                }
            }
        }
        out
    }

    /// What one shape makes of a position where its weight is `weight`: the height it adds
    /// to the terrain and as features, and how much of its full form rises there.
    fn rise(&self, shape: &Shape, x: f32, z: f32, weight: f32, ground: &Ground) -> (f32, f32, f32) {
        let [n0, n1, n2] = &shape.noises;
        match shape.kind {
            ShapeKind::Hills { height, spacing } => hills(weight, height, n0.get_noise_2d(x / spacing, z / spacing)),
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
                // Towers where a noise rises past a threshold, so each has its own ragged
                // outline and some merge into ridges; the threshold falls where the cluster
                // noise is high, and low knolls stand between them.
                let cluster = n1.get_noise_2d(x / (spacing * 6.0), z / (spacing * 6.0));
                let (wx, wz) = (x + 9.0 * n2.get_noise_2d(x / 23.0, z / 23.0), z + 9.0 * n2.get_noise_2d(x / 23.0 + 41.3, z / 23.0));
                let n = n0.get_noise_2d(wx / spacing, wz / spacing);
                let threshold = 0.45 - 0.2 * cluster;
                let tall = 0.35 + 0.65 * (0.5 + 0.5 * n1.get_noise_2d(x / (spacing * 1.3) + 7.1, z / (spacing * 1.3)));
                let tower = smoothstep(threshold, threshold + 0.05, n) * (0.75 + 0.25 * smoothstep(threshold, 0.95, n)) * tall;
                let knoll = 0.12 * smoothstep(threshold - 0.25, threshold - 0.1, n);
                let rise = tower.max(knoll);
                // The plain between the towers is flat.
                (-0.6 * ground.hills * weight, weight * height * rise, weight * rise)
            }
            ShapeKind::Spires { height, spacing } => {
                // Spikes where a noise rises past a threshold, tapering to points at its
                // peaks, in clusters; outlines ragged with a short warp.
                let cluster = n1.get_noise_2d(x / (spacing * 5.0), z / (spacing * 5.0));
                let (wx, wz) = (x + 4.0 * n2.get_noise_2d(x / 11.0, z / 11.0), z + 4.0 * n2.get_noise_2d(x / 11.0 + 17.3, z / 11.0));
                let n = n0.get_noise_2d(wx / spacing, wz / spacing);
                let threshold = 0.5 - 0.25 * cluster;
                let t = ((n - threshold) / (1.0 - threshold)).max(0.0);
                let tall = 0.35 + 0.65 * (0.5 + 0.5 * n1.get_noise_2d(x / (spacing * 1.7) + 3.3, z / (spacing * 1.7)));
                let spike = t.powf(0.55).min(1.0) * tall;
                (0.0, weight * height * spike, weight * spike)
            }
            ShapeKind::Volcanoes { height, radius } => {
                // A clean cone: the hills under it are smoothed away towards its summit.
                let (rise, summit) = self.volcano(shape, x, z, height, radius, ground);
                (rise - ground.hills * smoothstep(0.0, 0.5, summit), 0.0, summit)
            }
            ShapeKind::SeaCliffs { height } => {
                // Upland held high right to the shore, then falling sheer to the sea.
                let c = ground.relief.continental;
                let rim = smoothstep(COAST - 0.002, COAST + 0.003, c) * (1.0 - smoothstep(COAST + 0.02, COAST + 0.12, c));
                let tall = weight * height * (0.6 + 0.4 * n0.get_noise_2d(x / 300.0, z / 300.0));
                // Stacks left standing offshore as the cliffs wore back, where a noise rises
                // past a threshold: ragged outlines, some joined into reefs.
                let offshore = smoothstep(COAST - 0.035, COAST - 0.022, c) * (1.0 - smoothstep(COAST - 0.007, COAST - 0.002, c));
                let mut stack = 0.0;
                if offshore > 0.0 {
                    let (wx, wz) = (x + 5.0 * n1.get_noise_2d(x / 9.0, z / 9.0), z + 5.0 * n1.get_noise_2d(x / 9.0 + 23.0, z / 9.0));
                    let n = n2.get_noise_2d(wx / 34.0, wz / 34.0);
                    let pillar = smoothstep(0.55, 0.6, n);
                    let top = SEA_LEVEL as f32 + tall * (0.35 + 0.6 * (0.5 + 0.5 * n1.get_noise_2d(x / 60.0, z / 60.0))) * offshore;
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
            ShapeKind::Arches { span, height } => {
                // Fins of rock along the zero line of a noise, which come and go along it;
                // where a second noise rises along a fin, a window opens under it (`window`).
                // The noise's typical slope where it crosses zero (about 3 per unit) turns
                // its value into blocks from the line.
                let scale = span * 5.0;
                let d = n0.get_noise_2d(x / scale, z / scale).abs() / (3.0 / scale);
                let wide = 2.0 + 1.5 * (0.5 + 0.5 * n2.get_noise_2d(x / 40.0, z / 40.0));
                let fin = (1.0 - smoothstep(wide - 1.0, wide + 0.5, d)) * smoothstep(0.05, 0.25, n1.get_noise_2d(x / (span * 3.0), z / (span * 3.0)));
                // A fin stands full height from halfway into its climate: a low fin reads
                // as a wall, not an arch.
                let rise = smoothstep(0.0, 0.5, weight) * height * fin;
                (0.0, rise, weight * fin)
            }
        }
    }

    /// How high a window opens under a fin of an arch at (x, z), from the ground, given
    /// the fin's height there: all of it but the top four blocks, which carry it, where a
    /// noise rises along the fin, the opening rounding over towards its ends like an arch.
    fn window(&self, shape: &Shape, x: f32, z: f32, fin: f32) -> f32 {
        let ShapeKind::Arches { span, .. } = shape.kind else { return 0.0 };
        if fin < 9.0 {
            return 0.0;
        }
        let open = smoothstep(0.0, 0.4, shape.noises[2].get_noise_2d(x / span + 91.0, z / span));
        (open.sqrt() * (fin - 4.0)).floor()
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
        let units = std::array::from_fn(&unit);
        let centre = [(cx as f32 + 0.3 + 0.4 * unit(1)) * cell, (cz as f32 + 0.3 + 0.4 * unit(2)) * cell];
        // Out of its reach, the climate at its centre need not be weighed. (A distance is at
        // least its larger part, which spares most positions the root.)
        let (dx, dz) = (x - centre[0], z - centre[1]);
        if dx.abs() >= reach || dz.abs() >= reach || dx.hypot(dz) >= reach {
            return None;
        }
        // Outside the shape's regions (a volcano's fields), before the dearer climate.
        let region = match &shape.region {
            Some((noise, share)) => shape.region_share(*share, noise.get_noise_2d(centre[0], centre[1])),
            None => 1.0,
        };
        if region <= 0.0 {
            return None;
        }
        let regional = (ground.regional)(centre[0] as i32, centre[1] as i32);
        let fields = [regional.temperature(), regional.moisture(), regional.continentalness, regional.erosion];
        // On land: a cone rising from the sea would show as a ring of reefs.
        let weight = region * shape.climate_weight(centre[0], centre[1], &fields, &regional) * smoothstep(0.05, 0.25, regional.continentalness);
        (weight > 0.2).then_some(Standalone { centre, cell: [cx, cz], units, weight })
    }

    /// A volcano's cone at (x, z), if one stands there: the height it adds and how near its
    /// summit the position is, 0 ..= 1.
    fn volcano(&self, shape: &Shape, x: f32, z: f32, height: f32, radius: f32, ground: &Ground) -> (f32, f32) {
        let Some(cone) = self.standalone(shape, x, z, radius * 6.0, radius * 1.25 * (1.0 + WANDER), ground) else { return (0.0, 0.0) };
        let height = height * cone.weight * (0.75 + 0.4 * self.volcanism);
        let u = cone.units;
        let r = radius * (0.75 + 0.5 * u[3]);
        let Some((t, cos, sin, d)) = cone.reach(shape, x, z, r, WANDER) else { return (0.0, 0.0) };
        // Concave flanks that meet the land without a crease, and a crater with a rim.
        let flank = |t: f32| (1.0 - t).powf(1.7) * smootherstep(0.0, 0.35, 1.0 - t);
        let crater = 0.14 + 0.12 * u[4];
        let rim = flank(crater);
        let floor = rim - 0.12 - 0.2 * u[5];
        let mut h = if t >= crater { flank(t) } else { floor + (rim - floor) * smoothstep(crater * 0.6, crater, t) };
        // The rim breached on one side, where lava once poured out: a horseshoe crater.
        let (bx, bz) = (u[7] * std::f32::consts::TAU).sin_cos();
        let breach = smoothstep(0.6, 0.92, cos * bz + sin * bx) * (1.0 - smoothstep(crater, crater * 2.2, t));
        h -= (rim - floor) * 0.85 * breach;
        // Shallow gullies down the flanks.
        let k = r / 22.0;
        let gully = shape.noises[1].get_noise_3d(cos * k, sin * k, d / 40.0).abs();
        h *= 1.0 - 0.04 * (1.0 - gully) * smoothstep(crater, crater + 0.1, t);
        (height * (0.55 + 0.45 * u[6]) * h, 1.0 - t)
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
    /// Where (x, z) lies on a shape of radius `r` whose outline wanders by up to `wander`
    /// of it: its distance from the centre over the radius, the direction from the centre,
    /// and the distance, all measured in warped space. `None` outside it. The warp is a
    /// noise about as wide as the shape, so its contours bend into irregular lobes rather
    /// than the rays of a star that an outline wandering with the angle draws.
    fn reach(&self, shape: &Shape, x: f32, z: f32, r: f32, wander: f32) -> Option<(f32, f32, f32, f32)> {
        let (dx, dz) = (x - self.centre[0], z - self.centre[1]);
        let outside = r * (1.0 + wander);
        if dx.abs() >= outside || dz.abs() >= outside || dx.hypot(dz) >= outside {
            return None;
        }
        let (ox, oz) = (self.cell[0] as f32 * 17.0, self.cell[1] as f32 * 17.0);
        let (u, v) = (ox + 0.55 * dx / r, oz + 0.55 * dz / r);
        let warp = |noise: &FastNoiseLite, at: (f32, f32)| wander * r * noise.get_noise_2d(at.0, at.1).clamp(-1.0, 1.0);
        let (dx, dz) = (dx + warp(&shape.noises[0], (u, v)), dz + warp(&shape.noises[2], (u + 31.7, v - 47.3)));
        let d = dx.hypot(dz);
        let t = d / r;
        let (cos, sin) = if d > 0.0 { (dx / d, dz / d) } else { (1.0, 0.0) };
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
            _ => true,
        }
    }

    /// How strongly the shape rises at a position, 0 ..= 1, from its climate gates, rock
    /// province and rarity. (Generation weighs them apart, the cheaper first.)
    #[cfg(test)]
    fn weight(&self, x: f32, z: f32, fields: &[f32; 4], regional: &Regional) -> f32 {
        let weight = self.climate_weight(x, z, fields, regional);
        match &self.region {
            Some((noise, share)) if weight > 0.0 => weight * self.region_share(*share, noise.get_noise_2d(x, z)),
            _ => weight,
        }
    }

    /// How strongly the shape's climate gates and rock province let it rise at a position,
    /// 0 ..= 1: its weight before its rarity takes some regions away (`region_share`).
    fn climate_weight(&self, x: f32, z: f32, fields: &[f32; 4], regional: &Regional) -> f32 {
        let mut weight = if self.gates.is_empty() { 1.0 } else { self.gates.iter().map(|g| g.weight(fields)).fold(0.0, f32::max) };
        if weight > 0.0
            && let Some(province) = &self.province
        {
            weight *= if province.contains(x, z, regional.warp) { smoothstep(0.0, PROVINCE_RAMP, province.depth(x, z, regional.warp)) } else { 0.0 };
        }
        weight
    }

    /// How much of the shape a region whose mask noise is `mask` takes, 0 ..= 1, for a
    /// shape that takes `share` of its climate's regions.
    fn region_share(&self, share: f32, mask: f32) -> f32 {
        let u = climate::uniform(mask);
        let fade = (share / 2.0).min(0.03);
        smoothstep(1.0 - share, 1.0 - share + fade, u)
    }
}

/// What rolling hills of the given height make of a position where their weight is
/// `weight` and their noise `n`: folded noise, round tops with creased hollows between them.
fn hills(weight: f32, height: f32, n: f32) -> (f32, f32, f32) {
    (weight * height * (2.0 * n.abs()).min(1.5), 0.0, weight)
}

/// The noises each kind reads, at frequency 1 (callers scale positions themselves).
fn kind_noises(kind: ShapeKind, seed: u64) -> [FastNoiseLite; 3] {
    let octaves = match kind {
        ShapeKind::Hills { .. } | ShapeKind::Mesas { .. } | ShapeKind::Canyons { .. } => 2,
        _ => 1,
    };
    [noise(seed, 0x5a7e, 1.0, FractalType::FBm, octaves), noise(seed, 0x5a7f, 1.0, FractalType::FBm, 2), noise(seed, 0x5a80, 1.0, FractalType::FBm, 1)]
}

/// The chance that a cell of a standalone shape's grid holds one where it may, and the
/// share of the world's regions it may: about one volcano per fifteen square kilometres
/// in a moderately volcanic world, at rare, gathered into volcanic fields where most cells
/// hold one, so a field of cones is a place rather than cones scattered everywhere.
fn standalone_chance(kind: ShapeKind, share: f32, dials: &WorldDials) -> (f32, f32) {
    let base = match kind {
        ShapeKind::Volcanoes { .. } => 0.15 * (0.15 + 1.4 * dials.volcanism),
        _ => 1.0,
    };
    let overall = (base * share / Rarity::Rare.multiplier()).min(1.0);
    let fields = (overall / 0.6).clamp(0.05, 0.6);
    ((overall / fields).min(0.9), fields)
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
