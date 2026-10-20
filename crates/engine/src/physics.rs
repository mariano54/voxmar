//! Axis-aligned collision against the voxel grid, and voxel raycasting.

use glam::{IVec3, Vec3};

use crate::coords::Face;

/// Shared player dimensions (blocks). How fast players move is content: see
/// [`crate::movement::Tuning`].
pub mod player {
    pub const WIDTH: f32 = 0.6;
    pub const HEIGHT: f32 = 1.8;
    pub const EYE_HEIGHT: f32 = 1.62;
    pub const REACH: f32 = 5.0;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// A box standing on `feet`, centred horizontally.
    pub fn standing(feet: Vec3, width: f32, height: f32) -> Self {
        let half = Vec3::new(width / 2.0, 0.0, width / 2.0);
        Self { min: feet - half, max: feet + half + Vec3::Y * height }
    }

    pub fn translate(&mut self, delta: Vec3) {
        self.min += delta;
        self.max += delta;
    }

    /// Whether the box overlaps any cell for which `solid` returns true.
    pub fn intersects(&self, solid: impl Fn(IVec3) -> bool) -> bool {
        let lo = self.min.to_array().map(first_cell);
        let hi = self.max.to_array().map(last_cell);
        (lo[1]..=hi[1]).any(|y| (lo[2]..=hi[2]).any(|z| (lo[0]..=hi[0]).any(|x| solid(IVec3::new(x, y, z)))))
    }
}

/// How far a box may overlap a cell, or fall short of it, and still only touch it: a
/// tenth of a millimetre near the origin, and eight steps of f32 rounding further out,
/// where positions round to coarser steps (a quarter of a millimetre beyond 2,048 blocks).
/// A fixed tolerance there let rounding put a box a hair inside the wall it was stopped
/// at, which then counted it as already past, so walls far from the origin could be
/// walked through.
fn tolerance(at: f32) -> f32 {
    EPSILON.max(at.abs() * 8.0 * f32::EPSILON)
}

const EPSILON: f32 = 1e-4;

/// The first cell a box starting at `min` overlaps, along one axis.
fn first_cell(min: f32) -> i32 {
    (min + tolerance(min)).floor() as i32
}

/// The last cell a box ending at `max` overlaps, along one axis.
fn last_cell(max: f32) -> i32 {
    (max - tolerance(max)).floor() as i32
}

/// Moves `aabb` by `delta`, stopping at solid cells one axis at a time (Y, then X, then Z)
/// so movement slides along walls. Returns which axes were blocked.
pub fn move_and_slide(aabb: &mut Aabb, delta: Vec3, solid: impl Fn(IVec3) -> bool) -> [bool; 3] {
    let mut blocked = [false; 3];
    for axis in [1, 0, 2] {
        blocked[axis] = move_axis(aabb, axis, delta[axis], &solid);
    }
    blocked
}

fn move_axis(aabb: &mut Aabb, axis: usize, delta: f32, solid: &impl Fn(IVec3) -> bool) -> bool {
    if delta == 0.0 {
        return false;
    }
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let u_range = first_cell(aabb.min[u])..=last_cell(aabb.max[u]);
    let v_range = first_cell(aabb.min[v])..=last_cell(aabb.max[v]);
    let blocked_at = |c: i32| {
        u_range.clone().any(|a| {
            v_range.clone().any(|b| {
                let mut cell = IVec3::ZERO;
                cell[axis] = c;
                cell[u] = a;
                cell[v] = b;
                solid(cell)
            })
        })
    };

    let mut allowed = delta;
    if delta > 0.0 {
        let first = last_cell(aabb.max[axis]) + 1;
        let last = (aabb.max[axis] + delta).ceil() as i32 - 1;
        if let Some(c) = (first..=last).find(|&c| blocked_at(c)) {
            allowed = (c as f32 - aabb.max[axis]).max(0.0);
        }
    } else {
        let first = first_cell(aabb.min[axis]) - 1;
        let last = (aabb.min[axis] + delta).floor() as i32;
        if let Some(c) = (last..=first).rev().find(|&c| blocked_at(c)) {
            allowed = ((c + 1) as f32 - aabb.min[axis]).min(0.0);
        }
    }
    let mut offset = Vec3::ZERO;
    offset[axis] = allowed;
    aabb.translate(offset);
    allowed != delta
}

/// A whole cell: what a solid block that is not a shape stops movement with.
pub const CUBE: [Aabb; 1] = [Aabb { min: Vec3::ZERO, max: Vec3::ONE }];

impl Aabb {
    /// Whether the box overlaps any of the boxes `boxes` gives for the cells it is in, each in
    /// blocks from its cell's lower corner (a slab's lower half, a whole cube, nothing).
    pub fn overlaps<'a>(&self, boxes: impl Fn(IVec3) -> &'a [Aabb]) -> bool {
        let lo = self.min.to_array().map(first_cell);
        let hi = self.max.to_array().map(last_cell);
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    let cell = IVec3::new(x, y, z);
                    let at = cell.as_vec3();
                    if boxes(cell).iter().any(|b| (0..3).all(|k| apart(self.min[k], self.max[k], at[k] + b.min[k], at[k] + b.max[k]).is_none())) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Whether the span `lo..hi` of a moving box and `a..b` of a still one are apart along an
/// axis (`Some`), or overlap by more than rounding (`None`).
fn apart(lo: f32, hi: f32, a: f32, b: f32) -> Option<()> {
    (a >= hi - tolerance(hi) || b <= lo + tolerance(lo)).then_some(())
}

/// [`move_and_slide`] against the boxes `boxes` gives for each cell, in blocks from the
/// cell's lower corner, rather than whole cells: a slab stops a box at its top, a post at its
/// sides. A box it already overlaps (by more than rounding) does not stop it, as a whole cell
/// it is already in does not stop [`move_and_slide`]: a body a block was placed on moves out
/// of it rather than sticking (and may drop through it to what is below).
pub fn move_and_slide_boxes<'a>(aabb: &mut Aabb, delta: Vec3, boxes: impl Fn(IVec3) -> &'a [Aabb]) -> [bool; 3] {
    let mut blocked = [false; 3];
    for axis in [1, 0, 2] {
        blocked[axis] = move_axis_boxes(aabb, axis, delta[axis], &boxes);
    }
    blocked
}

/// Moves `aabb` by `delta` along `axis` until it meets one of the boxes `boxes` gives (see
/// [`move_and_slide_boxes`]); whether it was stopped short.
pub fn move_axis_boxes<'a>(aabb: &mut Aabb, axis: usize, delta: f32, boxes: &impl Fn(IVec3) -> &'a [Aabb]) -> bool {
    if delta == 0.0 {
        return false;
    }
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    // The cells the box sweeps: those it overlaps across, from the one its leading side is
    // in (a box there may still be ahead of it) to the one it would end in.
    let (from, to) = if delta > 0.0 {
        (last_cell(aabb.max[axis]), (aabb.max[axis] + delta).ceil() as i32 - 1)
    } else {
        ((aabb.min[axis] + delta).floor() as i32, first_cell(aabb.min[axis]))
    };
    let mut allowed = delta;
    for c in from..=to {
        for a in first_cell(aabb.min[u])..=last_cell(aabb.max[u]) {
            for b in first_cell(aabb.min[v])..=last_cell(aabb.max[v]) {
                let mut cell = IVec3::ZERO;
                cell[axis] = c;
                cell[u] = a;
                cell[v] = b;
                let at = cell.as_vec3();
                for bx in boxes(cell) {
                    let (lo, hi) = (at + bx.min, at + bx.max);
                    if apart(aabb.min[u], aabb.max[u], lo[u], hi[u]).is_some() || apart(aabb.min[v], aabb.max[v], lo[v], hi[v]).is_some() {
                        continue;
                    }
                    if delta > 0.0 && lo[axis] >= aabb.max[axis] - tolerance(aabb.max[axis]) {
                        allowed = allowed.min((lo[axis] - aabb.max[axis]).max(0.0));
                    } else if delta < 0.0 && hi[axis] <= aabb.min[axis] + tolerance(aabb.min[axis]) {
                        allowed = allowed.max((hi[axis] - aabb.min[axis]).min(0.0));
                    }
                }
            }
        }
    }
    let mut offset = Vec3::ZERO;
    offset[axis] = allowed;
    aabb.translate(offset);
    allowed != delta
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub block: IVec3,
    /// Face of `block` the ray entered through.
    pub face: Face,
    pub distance: f32,
}

/// Walks the grid cells along a ray (Amanatides & Woo) and returns the first cell for
/// which `hit` is true.
pub fn raycast(origin: Vec3, direction: Vec3, max_distance: f32, hit: impl Fn(IVec3) -> bool) -> Option<RayHit> {
    let dir = direction.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let mut cell = origin.floor().as_ivec3();
    let step = dir.signum().as_ivec3();
    let delta = (1.0 / dir).abs();
    let next_boundary = |o: f32, c: i32, d: f32| if d > 0.0 { c as f32 + 1.0 - o } else { o - c as f32 };
    let mut t_max = Vec3::new(
        next_boundary(origin.x, cell.x, dir.x) * delta.x,
        next_boundary(origin.y, cell.y, dir.y) * delta.y,
        next_boundary(origin.z, cell.z, dir.z) * delta.z,
    );
    let mut distance = 0.0;
    let mut normal = IVec3::ZERO;
    while distance <= max_distance {
        if hit(cell) && normal != IVec3::ZERO {
            return Some(RayHit { block: cell, face: Face::from_normal(normal)?, distance });
        }
        let axis = if t_max.x < t_max.y && t_max.x < t_max.z {
            0
        } else if t_max.y < t_max.z {
            1
        } else {
            2
        };
        distance = t_max[axis];
        t_max[axis] += delta[axis];
        cell[axis] += step[axis];
        normal = IVec3::ZERO;
        normal[axis] = -step[axis];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor_at_zero(c: IVec3) -> bool {
        c.y < 0
    }

    #[test]
    fn falling_box_lands_on_floor() {
        let mut aabb = Aabb::standing(Vec3::new(0.5, 3.0, 0.5), 0.6, 1.8);
        let blocked = move_and_slide(&mut aabb, Vec3::new(0.0, -10.0, 0.0), floor_at_zero);
        assert!(blocked[1]);
        assert!(aabb.min.y.abs() < 1e-5);
        // Resting on the floor, horizontal movement is free.
        let blocked = move_and_slide(&mut aabb, Vec3::new(3.0, -0.01, 0.0), floor_at_zero);
        assert_eq!(blocked, [false, true, false]);
        assert!((aabb.min.x - 3.2).abs() < 1e-5);
    }

    #[test]
    fn wall_stops_horizontal_motion() {
        let mut aabb = Aabb::standing(Vec3::new(0.5, 0.0, 0.5), 0.6, 1.8);
        let blocked = move_and_slide(&mut aabb, Vec3::new(5.0, 0.0, 0.0), |c| c.x == 2);
        assert!(blocked[0]);
        assert!((aabb.max.x - 2.0).abs() < 1e-5);
    }

    /// Far from the origin, f32 positions round to steps larger than a fixed tolerance;
    /// walking into a wall there must still stop at it, however long it is pushed.
    #[test]
    fn walls_far_from_the_origin_hold() {
        for base in [0.0f32, 1_100.0, 2_100.0, -3_000.0, 20_000.0] {
            let wall = base.floor() as i32 + 2;
            let solid = move |c: IVec3| c.y < 0 || (c.y <= 1 && c.x >= wall);
            let mut aabb = Aabb::standing(Vec3::new(base + 1.0, 0.0, base + 0.5), 0.6, 1.8);
            for _ in 0..1000 {
                // Rebuilt from the centre each step, as a player's box is.
                let feet = Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0);
                aabb = Aabb::standing(feet, 0.6, 1.8);
                move_and_slide(&mut aabb, Vec3::new(0.04, -0.01, 0.012), solid);
            }
            assert!(aabb.max.x <= wall as f32 + tolerance(wall as f32), "through the wall at {base}: {}", aabb.max.x);
            assert!(aabb.max.x > wall as f32 - 0.01, "stopped short at {base}");
        }
    }

    /// Against whole cubes, moving by boxes goes exactly as moving by cells does.
    #[test]
    fn boxes_of_whole_cubes_move_as_cells() {
        let mut seed = 0x9e37_79b9_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        for base in [0.0f32, 2_100.0, -3_000.0] {
            let b = base.floor() as i32;
            let solid = move |c: IVec3| c.y < 0 || ((c.x - b) * 7 + c.z * 3 + c.y * 5).rem_euclid(11) == 0;
            let boxes = |c: IVec3| -> &[Aabb] { if solid(c) { &CUBE } else { &[] } };
            for _ in 0..400 {
                let feet = Vec3::new(base + next() * 8.0, next() * 3.0, next() * 8.0);
                let (mut cells, mut shaped) = (Aabb::standing(feet, 0.6, 1.8), Aabb::standing(feet, 0.6, 1.8));
                if cells.intersects(solid) {
                    continue;
                }
                let delta = Vec3::new(next() - 0.5, next() - 0.6, next() - 0.5) * 3.0;
                assert_eq!(move_and_slide(&mut cells, delta, solid), move_and_slide_boxes(&mut shaped, delta, boxes), "from {feet} by {delta}");
                assert_eq!(cells, shaped, "from {feet} by {delta}");
                assert_eq!(cells.intersects(solid), shaped.overlaps(boxes));
            }
        }
    }

    /// A box falls onto a slab's top, not its cell's, and walks under nothing in the slab's
    /// upper half; a post stops it at its side.
    #[test]
    fn boxes_stop_where_they_are() {
        const SLAB: [Aabb; 1] = [Aabb { min: Vec3::ZERO, max: Vec3::new(1.0, 0.5, 1.0) }];
        const POST: [Aabb; 1] = [Aabb { min: Vec3::new(0.375, 0.0, 0.375), max: Vec3::new(0.625, 1.0, 0.625) }];
        let boxes = |c: IVec3| -> &[Aabb] {
            match (c.x, c.y, c.z) {
                (_, y, _) if y < 0 => &CUBE,
                (0, 0, 0) => &SLAB,
                (3, 0, 0) => &POST,
                _ => &[],
            }
        };
        let mut aabb = Aabb::standing(Vec3::new(0.5, 3.0, 0.5), 0.6, 1.8);
        let blocked = move_and_slide_boxes(&mut aabb, Vec3::new(0.0, -10.0, 0.0), boxes);
        assert!(blocked[1]);
        assert!((aabb.min.y - 0.5).abs() < 1e-5, "landed at {}", aabb.min.y);
        assert!(!aabb.overlaps(boxes));
        // Off the slab and down to the ground, then along it into the post.
        move_and_slide_boxes(&mut aabb, Vec3::new(1.5, 0.0, 0.0), boxes);
        move_and_slide_boxes(&mut aabb, Vec3::new(0.0, -1.0, 0.0), boxes);
        assert!(aabb.min.y.abs() < 1e-5);
        let blocked = move_and_slide_boxes(&mut aabb, Vec3::new(3.0, 0.0, 0.0), boxes);
        assert!(blocked[0]);
        assert!((aabb.max.x - 3.375).abs() < 1e-5, "stopped at {}", aabb.max.x);
    }

    #[test]
    fn raycast_reports_entry_face() {
        let hit = raycast(Vec3::new(0.5, 5.5, 0.5), Vec3::NEG_Y, 10.0, |c| c.y == 0).unwrap();
        assert_eq!(hit.block, IVec3::new(0, 0, 0));
        assert_eq!(hit.face, Face::PosY);
        assert!((hit.distance - 4.5).abs() < 1e-5);
    }
}
