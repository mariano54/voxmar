//! Axis-aligned collision against the voxel grid, and voxel raycasting.

use glam::{IVec3, Vec3};

use crate::coords::Face;

/// Shared player dimensions and movement constants (blocks, seconds).
pub mod player {
    pub const WIDTH: f32 = 0.6;
    pub const HEIGHT: f32 = 1.8;
    pub const EYE_HEIGHT: f32 = 1.62;
    pub const GRAVITY: f32 = 32.0;
    pub const TERMINAL_VELOCITY: f32 = 78.0;
    pub const JUMP_VELOCITY: f32 = 9.0;
    pub const WALK_SPEED: f32 = 4.3;
    pub const SPRINT_SPEED: f32 = 5.6;
    pub const FLY_SPEED: f32 = 12.0;
    pub const FLY_SPRINT_SPEED: f32 = 32.0;
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
        let lo = (self.min + EPSILON).floor().as_ivec3();
        let hi = (self.max - EPSILON).floor().as_ivec3();
        (lo.y..=hi.y).any(|y| (lo.z..=hi.z).any(|z| (lo.x..=hi.x).any(|x| solid(IVec3::new(x, y, z)))))
    }
}

const EPSILON: f32 = 1e-4;

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
    let u_range = (aabb.min[u] + EPSILON).floor() as i32..=(aabb.max[u] - EPSILON).floor() as i32;
    let v_range = (aabb.min[v] + EPSILON).floor() as i32..=(aabb.max[v] - EPSILON).floor() as i32;
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
        let first = (aabb.max[axis] - EPSILON).floor() as i32 + 1;
        let last = (aabb.max[axis] + delta).ceil() as i32 - 1;
        if let Some(c) = (first..=last).find(|&c| blocked_at(c)) {
            allowed = (c as f32 - aabb.max[axis]).max(0.0);
        }
    } else {
        let first = (aabb.min[axis] + EPSILON).floor() as i32 - 1;
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

    #[test]
    fn raycast_reports_entry_face() {
        let hit = raycast(Vec3::new(0.5, 5.5, 0.5), Vec3::NEG_Y, 10.0, |c| c.y == 0).unwrap();
        assert_eq!(hit.block, IVec3::new(0, 0, 0));
        assert_eq!(hit.face, Face::PosY);
        assert!((hit.distance - 4.5).abs() < 1e-5);
    }
}
