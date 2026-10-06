//! Creature bodies as one smooth, low-poly mesh that bends at the joints (Mariano's pick,
//! 2026-10-06: "low poly mesh, pixel skin").
//!
//! The body is built from the same parts the box model uses ([`super::Part`]): each part
//! becomes a rounded solid (a box with its edges rounded away, nearly a capsule for legs
//! and necks), the solids are blended where they meet, and the surface is meshed coarsely
//! (surface nets on a grid of about 26 cells across the creature), so it comes out
//! faceted, with real volume and a clean silhouette. Every vertex is weighted to the parts
//! nearest it, so the animation that turns parts about their pivots bends the mesh.
//! Because it is made from the parts, every species (and any a mod or the generator makes
//! from numbers) gets a body without a model file.
//!
//! The skin is painted through the volume ([`super::skin::paint_volume`]) and snapped to
//! the game's pixel grid when drawn, so the surface matches the blocks.

use glam::Vec3;

use super::model::{Part, Role, Swing};

/// A body's mesh at rest, in model space (feet at the origin, facing −Z).
#[derive(Clone, Debug, Default)]
pub struct BodyMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// The four parts each vertex follows most, and how much.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// The box the body fills, for its skin's volume.
    pub min: Vec3,
    pub max: Vec3,
}

/// Cells across the body's longest side.
const CELLS: f32 = 26.0;
/// The smallest and largest cell, in blocks: tiny animals keep their detail, big ones stay
/// cheap.
const SMALLEST: f32 = 0.018;
const LARGEST: f32 = 0.075;

/// How much of its smallest half-thickness a part's edges are rounded by.
fn rounding(role: Role) -> f32 {
    match role {
        Role::Leg | Role::Neck | Role::Tail | Role::Arm | Role::Horn | Role::Stalk => 0.95,
        Role::Torso | Role::Hump | Role::Thigh | Role::Shell | Role::Carapace => 0.7,
        Role::Head | Role::Snout | Role::Beak => 0.55,
        Role::Ear | Role::Wing | Role::Crest => 0.5,
        Role::Foot => 0.4,
        Role::Eye | Role::Mote => 1.0,
    }
}

/// A part as a rounded solid: its signed distance at a point (negative inside).
#[derive(Clone, Copy, Debug)]
struct Solid {
    centre: Vec3,
    inverse: glam::Quat,
    half: Vec3,
    round: f32,
}

impl Solid {
    fn new(part: &Part, cell: f32) -> Self {
        // Nothing thinner than most of a cell, or the coarse grid loses it.
        let half = (part.size * 0.5).max(Vec3::splat(cell * 0.62));
        let round = half.min_element() * rounding(part.role);
        Self { centre: part.centre, inverse: part.rest.inverse(), half, round }
    }

    fn distance(&self, p: Vec3) -> f32 {
        let q = (self.inverse * (p - self.centre)).abs() - (self.half - Vec3::splat(self.round));
        q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - self.round
    }
}

/// Smooth minimum: two solids blended over `k` where they meet.
fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

/// The parts as solids, with what the body's surface is made of.
pub struct Shape {
    solids: Vec<Solid>,
    pub cell: f32,
    blend: f32,
}

impl Shape {
    pub fn new(parts: &[Part]) -> Self {
        let (min, max) = bounds(parts);
        let cell = ((max - min).max_element() / CELLS).clamp(SMALLEST, LARGEST);
        Self { solids: parts.iter().map(|p| Solid::new(p, cell)).collect(), cell, blend: cell * 1.1 }
    }

    /// The body's signed distance at `p`.
    pub fn distance(&self, p: Vec3) -> f32 {
        let mut d = f32::MAX;
        for solid in &self.solids {
            let s = solid.distance(p);
            d = if d == f32::MAX { s } else { smooth_min(d, s, self.blend) };
        }
        d
    }

    /// The part nearest `p`, and how far its surface is.
    pub fn nearest(&self, p: Vec3) -> Option<(usize, f32)> {
        self.solids.iter().enumerate().map(|(i, s)| (i, s.distance(p))).min_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// The box around every part.
fn bounds(parts: &[Part]) -> (Vec3, Vec3) {
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for part in parts {
        for corner in 0..8 {
            let sign = Vec3::new(if corner & 1 == 0 { -0.5 } else { 0.5 }, if corner & 2 == 0 { -0.5 } else { 0.5 }, if corner & 4 == 0 { -0.5 } else { 0.5 });
            let p = part.at_rest(sign * part.size);
            min = min.min(p);
            max = max.max(p);
        }
    }
    if min.x > max.x {
        return (Vec3::ZERO, Vec3::ONE);
    }
    (min, max)
}

/// Meshes the body: surface nets over the blended solids, every vertex weighted to its
/// nearest parts.
pub fn build(parts: &[Part]) -> BodyMesh {
    let shape = Shape::new(parts);
    let cell = shape.cell;
    let (lo, hi) = bounds(parts);
    let origin = lo - Vec3::splat(cell * 2.0);
    let size = ((hi - lo) / cell).ceil().as_ivec3() + glam::IVec3::splat(5);
    let (nx, ny, nz) = (size.x as usize, size.y as usize, size.z as usize);
    let at = |x: usize, y: usize, z: usize| origin + Vec3::new(x as f32, y as f32, z as f32) * cell;
    // Distances at every grid corner.
    let mut field = vec![0.0f32; nx * ny * nz];
    let index = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                field[index(x, y, z)] = shape.distance(at(x, y, z));
            }
        }
    }
    // A vertex in every cell the surface crosses, where its edges cross on average.
    const EDGES: [((usize, usize, usize), (usize, usize, usize)); 12] = [
        ((0, 0, 0), (1, 0, 0)), ((0, 1, 0), (1, 1, 0)), ((0, 0, 1), (1, 0, 1)), ((0, 1, 1), (1, 1, 1)),
        ((0, 0, 0), (0, 1, 0)), ((1, 0, 0), (1, 1, 0)), ((0, 0, 1), (0, 1, 1)), ((1, 0, 1), (1, 1, 1)),
        ((0, 0, 0), (0, 0, 1)), ((1, 0, 0), (1, 0, 1)), ((0, 1, 0), (0, 1, 1)), ((1, 1, 0), (1, 1, 1)),
    ];
    let mut vertex_of = vec![u32::MAX; nx * ny * nz];
    let mut mesh = BodyMesh::default();
    for z in 0..nz - 1 {
        for y in 0..ny - 1 {
            for x in 0..nx - 1 {
                let (mut sum, mut crossings) = (Vec3::ZERO, 0);
                for &((ax, ay, az), (bx, by, bz)) in &EDGES {
                    let (da, db) = (field[index(x + ax, y + ay, z + az)], field[index(x + bx, y + by, z + bz)]);
                    if (da < 0.0) != (db < 0.0) {
                        let t = da / (da - db);
                        sum += at(x + ax, y + ay, z + az).lerp(at(x + bx, y + by, z + bz), t);
                        crossings += 1;
                    }
                }
                if crossings == 0 {
                    continue;
                }
                let p = sum / crossings as f32;
                vertex_of[index(x, y, z)] = mesh.positions.len() as u32;
                mesh.positions.push(p.to_array());
            }
        }
    }
    // A quad across every grid edge the surface crosses, joining the four cells around it,
    // turned to face outward.
    for z in 1..nz - 1 {
        for y in 1..ny - 1 {
            for x in 1..nx - 1 {
                let here = field[index(x, y, z)] < 0.0;
                for axis in 0..3 {
                    let (dx, dy, dz) = [(1, 0, 0), (0, 1, 0), (0, 0, 1)][axis];
                    if x + dx >= nx || y + dy >= ny || z + dz >= nz {
                        continue;
                    }
                    let there = field[index(x + dx, y + dy, z + dz)] < 0.0;
                    if here == there {
                        continue;
                    }
                    // The four cells sharing this edge.
                    let cells: [(usize, usize, usize); 4] = match axis {
                        0 => [(x, y - 1, z - 1), (x, y, z - 1), (x, y, z), (x, y - 1, z)],
                        1 => [(x - 1, y, z - 1), (x, y, z - 1), (x, y, z), (x - 1, y, z)],
                        _ => [(x - 1, y - 1, z), (x, y - 1, z), (x, y, z), (x - 1, y, z)],
                    };
                    let v = cells.map(|(cx, cy, cz)| vertex_of[index(cx, cy, cz)]);
                    if v.contains(&u32::MAX) {
                        continue;
                    }
                    // Turned to face outward below, by the field.
                    mesh.indices.extend([v[0], v[1], v[2], v[0], v[2], v[3]]);
                }
            }
        }
    }
    // Every triangle turned to face out of the body: the way the field rises.
    for t in mesh.indices.chunks_mut(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from_array(mesh.positions[i as usize]));
        let centre = (a + b + c) / 3.0;
        let e = cell * 0.25;
        let rise = Vec3::new(
            shape.distance(centre + Vec3::X * e) - shape.distance(centre - Vec3::X * e),
            shape.distance(centre + Vec3::Y * e) - shape.distance(centre - Vec3::Y * e),
            shape.distance(centre + Vec3::Z * e) - shape.distance(centre - Vec3::Z * e),
        );
        if (b - a).cross(c - a).dot(rise) < 0.0 {
            t.swap(1, 2);
        }
    }
    // Normals from the field, and weights to the parts nearest each vertex.
    let soft = cell * 1.6;
    for p in &mesh.positions {
        let p = Vec3::from_array(*p);
        let e = cell * 0.5;
        let n = Vec3::new(
            shape.distance(p + Vec3::X * e) - shape.distance(p - Vec3::X * e),
            shape.distance(p + Vec3::Y * e) - shape.distance(p - Vec3::Y * e),
            shape.distance(p + Vec3::Z * e) - shape.distance(p - Vec3::Z * e),
        )
        .normalize_or(Vec3::Y);
        mesh.normals.push(n.to_array());
        let mut near: Vec<(usize, f32)> = shape.solids.iter().enumerate().map(|(i, s)| (i, s.distance(p))).collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        let best = near[0].1;
        let mut joints = [0u16; 4];
        let mut weights = [0.0f32; 4];
        for (k, &(i, d)) in near.iter().take(4).enumerate() {
            joints[k] = i as u16;
            weights[k] = (-(d - best).max(0.0) / soft).exp();
        }
        // Parts that do not move share the body's motion: no need to weight among them.
        let total: f32 = weights.iter().sum();
        mesh.joints.push(joints);
        mesh.weights.push(weights.map(|w| w / total));
    }
    let (min, max) = mesh.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| {
        let p = Vec3::from_array(*p);
        (a.min(p), b.max(p))
    });
    mesh.min = min;
    mesh.max = max;
    mesh
}

/// Whether a part moves with the animation (and so needs a joint of its own).
pub fn moves(part: &Part) -> bool {
    part.swing != Swing::Still
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature::model::{HornShape, Model};

    fn deer() -> Vec<Part> {
        Model::Quadruped {
            body: (0.5, 0.58, 1.0),
            head: (0.28, 0.28, 0.4),
            legs: 0.75,
            ears: Some((0.09, 0.16)),
            tail: Some((0.1, 0.12, 0.08)),
            horns: Some((0.06, 0.62)),
            horn_shape: HornShape::Antlers,
            snout: Some((0.16, 0.14, 0.12)),
            neck: Some((0.34, 50.0)),
            haunches: false,
            hump: None,
            shell: None,
            tail_droop: 65.0,
            ear_lean: 55.0,
        }
        .parts(1.0)
    }

    #[test]
    fn bodies_are_closed_low_poly_and_stand_on_the_ground() {
        let parts = deer();
        let mesh = build(&parts);
        let triangles = mesh.indices.len() / 3;
        assert!((500..8000).contains(&triangles), "{triangles} triangles");
        assert!(mesh.min.y > -0.08 && mesh.min.y < 0.06, "feet at {}", mesh.min.y);
        // Closed: no edge belongs to one triangle alone (thin parts may share an edge
        // among four, as surface nets do where two sheets meet).
        let mut edges = std::collections::HashMap::new();
        for t in mesh.indices.chunks(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        let open = edges.values().filter(|&&n| n % 2 == 1).count();
        assert_eq!(open, 0, "the surface has {open} open edges");
        // Weights sum to one, and outward faces: normals point the way the winding does.
        assert!(mesh.weights.iter().all(|w| (w.iter().sum::<f32>() - 1.0).abs() < 1e-4));
        let mut agree = 0;
        for t in mesh.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from_array(mesh.positions[i as usize]));
            let face = (b - a).cross(c - a);
            if face.dot(Vec3::from_array(mesh.normals[t[0] as usize])) > 0.0 {
                agree += 1;
            }
        }
        assert!(agree * 10 > mesh.indices.len() / 3 * 9, "{agree} of {} faces face out", mesh.indices.len() / 3);
    }

    #[test]
    fn legs_follow_their_own_part() {
        let parts = deer();
        let mesh = build(&parts);
        let leg = parts.iter().position(|p| p.role == Role::Leg).unwrap();
        // The lowest vertex under that leg is weighted to it.
        let foot = parts[leg].pivot.with_y(0.0);
        let (i, _) = mesh
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| (i, Vec3::from_array(*p).distance(foot)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert_eq!(mesh.joints[i][0] as usize, leg);
        assert!(mesh.weights[i][0] > 0.6);
    }
}
