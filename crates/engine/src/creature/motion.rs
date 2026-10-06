//! How a creature's body moves: walking toward what its brain wants, turning at its turn
//! rate, jumping up single steps, falling, floating in water, and drifting in the air for
//! creatures that float. The same box collision as players (`physics::move_and_slide`),
//! sized to the creature.

use glam::{IVec3, Vec3};

use super::{Movement, Style, Water};
use crate::physics::{Aabb, move_and_slide};

/// Blocks per second squared, as players fall.
const GRAVITY: f32 = 32.0;
const TERMINAL: f32 = 60.0;
/// Height above the ground a drifting creature floats at.
const DRIFT_HEIGHT: f32 = 1.6;

/// A creature's moving state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Feet, centred.
    pub position: Vec3,
    pub velocity: Vec3,
    /// Facing, radians, as players' yaw (0 looks along −Z).
    pub yaw: f32,
    pub on_ground: bool,
    pub in_water: bool,
    /// Width and height of its box.
    pub size: (f32, f32),
    /// Whether the last step was stopped by a wall across.
    pub against_wall: bool,
    /// How far its head reaches ahead of its centre, and how high it is: a deer's head is
    /// well out in front of its box, and must not go into rock.
    pub reach: f32,
    pub head: f32,
}

impl Body {
    pub fn new(position: Vec3, yaw: f32, size: (f32, f32)) -> Self {
        Self { position, velocity: Vec3::ZERO, yaw, on_ground: false, in_water: false, size, against_wall: false, reach: 0.0, head: 0.0 }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, self.size.0, self.size.1)
    }

    /// The direction it faces.
    pub fn forward(&self) -> Vec3 {
        Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos())
    }

    /// Whether its head (and the neck on the way to it), beyond its box, is in something
    /// solid.
    pub fn head_blocked(&self, world: &impl Surroundings) -> bool {
        if self.reach <= self.size.0 * 0.5 + 0.05 {
            return false;
        }
        let forward = self.forward();
        [(1.0, 1.0), (1.0, 0.7), (0.75, 0.85)].into_iter().any(|(out, up)| {
            let p = self.position + forward * self.reach * out + Vec3::Y * (self.head * up).max(0.2);
            world.solid(p.floor().as_ivec3())
        })
    }
}

/// What the brain asks of the body this step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Drive {
    /// Where to head (horizontal; for swimmers and drifters, any direction), unit length or
    /// zero to stand.
    pub wish: Vec3,
    /// Blocks per second.
    pub speed: f32,
    /// A direction to face while standing; walking faces the way it goes.
    pub face: Option<Vec3>,
}

/// What the body needs to know about the world.
pub trait Surroundings {
    fn solid(&self, cell: IVec3) -> bool;
    fn water(&self, cell: IVec3) -> bool;
}

/// The yaw that faces along `direction`.
pub fn yaw_of(direction: Vec3) -> f32 {
    (-direction.x).atan2(-direction.z)
}

/// Turns `from` toward `to` by at most `step` radians, the short way round.
fn turn(from: f32, to: f32, step: f32) -> f32 {
    let delta = (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    from + delta.clamp(-step, step)
}

/// Advances the body by `dt` seconds.
pub fn step(body: &mut Body, world: &impl Surroundings, movement: &Movement, water: Water, drive: Drive, dt: f32) {
    let feet = body.position.floor().as_ivec3();
    let middle = (body.position + Vec3::Y * body.size.1 * 0.5).floor().as_ivec3();
    body.in_water = world.water(feet) || world.water(middle);
    let floating = water == Water::Air || movement.style == Style::Drifter;

    let (start, start_yaw) = (body.position, body.yaw);
    // Face the way it moves, or what it was asked to look at.
    let heading = if drive.wish.length_squared() > 1e-4 { Some(drive.wish) } else { drive.face };
    if let Some(heading) = heading.filter(|h| h.x.abs() + h.z.abs() > 1e-4) {
        body.yaw = turn(body.yaw, yaw_of(heading), movement.turn.to_radians() * dt);
    }

    // Walk in the direction it faces once it roughly faces it, so it turns before it goes.
    let facing = body.forward();
    let along = drive.wish.with_y(0.0).normalize_or_zero();
    let aligned = along.dot(facing).max(0.0);
    let target = along * drive.speed * (0.3 + 0.7 * aligned);
    let control = if body.on_ground || body.in_water || floating { 10.0 } else { 2.0 };
    let horizontal = body.velocity.with_y(0.0).lerp(target.with_y(0.0), (control * dt).min(1.0));
    body.velocity = horizontal.with_y(body.velocity.y);

    if floating {
        // Float above the ground, bobbing, rising over what is in the way.
        let ground = (0..8).map(|d| feet - IVec3::Y * d).find(|&c| world.solid(c)).map_or(body.position.y - 8.0, |c| c.y as f32 + 1.0);
        let want = ground + DRIFT_HEIGHT + drive.wish.y * 2.0;
        body.velocity.y = ((want - body.position.y) * 2.0).clamp(-2.0, 2.0);
    } else if body.in_water && water == Water::Swim {
        body.velocity.y = body.velocity.y * (1.0 - (3.0 * dt).min(1.0)) + drive.wish.y * drive.speed * 3.0 * dt;
    } else if body.in_water {
        // Bobs up to the surface and paddles.
        body.velocity.y = (body.velocity.y + 18.0 * dt).min(2.5);
    } else {
        body.velocity.y = (body.velocity.y - GRAVITY * dt).max(-TERMINAL);
    }

    // Hop up a step it walks into, if it can jump and there is room above.
    if body.on_ground && body.against_wall && movement.jump > 0.0 && along != Vec3::ZERO {
        let mut raised = body.aabb();
        raised.translate(Vec3::Y * 1.05 + along * 0.3);
        if !raised.intersects(|p| world.solid(p)) {
            body.velocity.y = movement.jump;
        }
    }
    // Hoppers move in hops whenever they move.
    if movement.style == Style::Hopper && body.on_ground && target.length_squared() > 0.05 && body.velocity.y <= 0.0 {
        body.velocity.y = movement.jump.min(6.0) * 0.75;
    }

    let mut aabb = body.aabb();
    let blocked = move_and_slide(&mut aabb, body.velocity * dt, |p| world.solid(p));
    body.on_ground = blocked[1] && body.velocity.y <= 0.0;
    body.against_wall = blocked[0] || blocked[2];
    for (axis, blocked) in blocked.into_iter().enumerate() {
        if blocked {
            body.velocity[axis] = 0.0;
        }
    }
    body.position = Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0);
    // Its head stops at what its box does not: back out of the turn or the step that put it
    // into a wall, and count it as against the wall, so it climbs or turns away.
    if body.head_blocked(world) {
        body.yaw = start_yaw;
        let back = Vec3::new(start.x, body.position.y, start.z);
        if !Aabb::standing(back, body.size.0, body.size.1).intersects(|p| world.solid(p)) {
            body.position = back;
        }
        body.velocity.x = 0.0;
        body.velocity.z = 0.0;
        body.against_wall = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Flat {
        wall: Option<i32>,
    }

    impl Surroundings for Flat {
        fn solid(&self, cell: IVec3) -> bool {
            cell.y < 0 || (self.wall.is_some_and(|x| cell.x >= x) && cell.y == 0)
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    fn walk(body: &mut Body, world: &Flat, wish: Vec3, seconds: f32) {
        let movement = Movement::default();
        for _ in 0..(seconds * 20.0) as usize {
            step(body, world, &movement, Water::Land, Drive { wish, speed: 2.0, face: None }, 0.05);
        }
    }

    #[test]
    fn walks_turns_and_lands() {
        let world = Flat { wall: None };
        let mut body = Body::new(Vec3::new(0.5, 2.0, 0.5), 0.0, (0.6, 0.8));
        walk(&mut body, &world, Vec3::X, 3.0);
        assert!(body.on_ground && body.position.y.abs() < 1e-4);
        assert!(body.position.x > 3.5, "walked to {}", body.position.x);
        assert!((body.forward() - Vec3::X).length() < 0.05, "faces where it goes");
    }

    #[test]
    fn heads_stop_at_walls() {
        // A wall two blocks high: the box would reach it, the head first.
        struct Wall;
        impl Surroundings for Wall {
            fn solid(&self, cell: IVec3) -> bool {
                cell.y < 0 || (cell.x >= 3 && (0..3).contains(&cell.y))
            }
            fn water(&self, _: IVec3) -> bool {
                false
            }
        }
        let mut body = Body::new(Vec3::new(0.5, 0.0, 0.5), -std::f32::consts::FRAC_PI_2, (0.6, 1.4));
        body.reach = 0.9;
        body.head = 1.4;
        let movement = Movement { jump: 0.0, ..Movement::default() };
        for _ in 0..80 {
            step(&mut body, &Wall, &movement, Water::Land, Drive { wish: Vec3::X, speed: 2.0, face: None }, 0.05);
        }
        assert!(body.position.x + body.reach <= 3.0 + 1e-3, "the head went into the wall at {}", body.position.x + body.reach);
        assert!(body.against_wall);
    }

    #[test]
    fn steps_up_a_block() {
        let world = Flat { wall: Some(3) };
        let mut body = Body::new(Vec3::new(0.5, 0.0, 0.5), -std::f32::consts::FRAC_PI_2, (0.6, 0.8));
        walk(&mut body, &world, Vec3::X, 4.0);
        assert!(body.position.x > 3.5 && body.position.y >= 1.0 - 1e-3, "stuck at {}", body.position);
    }
}
