//! Dropped items: stacks lying loose in the world, as Minecraft's item entities. A broken
//! block pops its drops out with a little hop, a thrown stack flies from the hand, and
//! both fall, bounce, slide to rest and float in water, until a player walks close enough
//! to draw them in.
//!
//! The server owns them (see `server::items`); this is the motion they share with anything
//! that wants to predict it. Numbers follow Minecraft's item entity, converted from ticks
//! to seconds.

use glam::{IVec3, Vec3};

use crate::movement::Surroundings;
use crate::physics::{Aabb, move_and_slide, player};

/// Width and height of a dropped item's box (Minecraft's 0.25).
pub const SIZE: f32 = 0.25;
/// Blocks per second squared (Minecraft's 0.04 a tick).
pub const GRAVITY: f32 = 16.0;
/// How quickly speed decays in the air, per second (Minecraft keeps 0.98 a tick).
pub const AIR_DRAG: f32 = 0.4;
/// How quickly an item sliding on the ground slows, per second (Minecraft keeps 0.588 a
/// tick on most blocks).
pub const GROUND_DRAG: f32 = 10.6;
/// Of the speed an item lands with, how much it bounces back up (Minecraft's half).
pub const BOUNCE: f32 = 0.5;
/// Landings slower than this, in blocks per second, don't bounce: the item settles.
const SETTLE: f32 = 1.5;
/// Upward push in water, against gravity, so items rise to the surface and bob there.
const BUOYANCY: f32 = 22.0;
/// How quickly water slows an item, per second.
const WATER_DRAG: f32 = 3.0;
/// Speeds below this, in blocks per second, count as resting.
const REST: f32 = 0.01;

/// Seconds after a block breaks before its drops can be picked up (Minecraft's ten
/// ticks), so they are seen to pop out first.
pub const PICKUP_DELAY: f32 = 0.5;
/// Seconds after a player throws a stack before anyone can pick it up (Minecraft's two
/// seconds), so it isn't caught again at once.
pub const THROWN_PICKUP_DELAY: f32 = 2.0;
/// Seconds of world time a dropped item lasts before it vanishes: half an hour (Mariano,
/// 2026-10-06), whether or not anyone is near to keep its ground loaded.
pub const LIFETIME: f32 = 1800.0;
/// Items within this many blocks of a player's middle, and free to be picked up, drift
/// towards them.
pub const MAGNET: f32 = 2.5;
/// Players within this many blocks of an item, middle to its centre, are in the running
/// for it: the nearest of them with room for it is the one it goes to.
pub const CLAIM: f32 = MAGNET + 1.0;
/// How far a player's box reaches for items, across and up and down (Minecraft's 1 and
/// 0.5).
pub const REACH: Vec3 = Vec3::new(1.0, 0.5, 1.0);

/// Speed a thrown stack leaves the hand at, in blocks per second (Minecraft's 0.3 a tick).
pub const THROW_SPEED: f32 = 6.0;

/// What a player does to an item free to be picked up that is theirs to have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    /// Nothing: it lies where it is.
    Left,
    /// Drifts towards them.
    Drawn,
    /// Goes into their inventory.
    Taken,
}

/// The middle of a player standing at `feet`, where items are measured from.
pub fn middle(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * player::HEIGHT * 0.5
}

/// The moving state of a dropped item.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Loose {
    /// Bottom centre of its box.
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
}

impl Loose {
    pub fn new(position: Vec3, velocity: Vec3) -> Self {
        Self { position, velocity, on_ground: false }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, SIZE, SIZE)
    }

    pub fn centre(&self) -> Vec3 {
        self.position + Vec3::Y * SIZE / 2.0
    }

    /// What a player standing at `feet` does to it: takes it where it touches their box
    /// grown by `REACH`, draws it in within `MAGNET` of their middle, and otherwise leaves
    /// it. The server's one rule, which a test waiting out a pickup asks too.
    pub fn pull(&self, feet: Vec3) -> Pull {
        let mut reach = Aabb::standing(feet, player::WIDTH, player::HEIGHT);
        reach.min -= REACH;
        reach.max += REACH;
        let body = self.aabb();
        if body.min.cmple(reach.max).all() && body.max.cmpge(reach.min).all() {
            Pull::Taken
        } else if middle(feet).distance(self.centre()) < MAGNET {
            Pull::Drawn
        } else {
            Pull::Left
        }
    }

    /// Lying still on the ground: nothing moves it until the ground goes or something
    /// pushes it.
    pub fn resting(&self) -> bool {
        self.on_ground && self.velocity.length_squared() < REST * REST
    }

    /// Advances the item by `dt` seconds. Returns whether it moved.
    pub fn step(&mut self, world: &impl Surroundings, dt: f32) -> bool {
        let (depth, current) = world.water(&self.aabb());
        let floating = depth > 0.1 * SIZE;
        if self.resting() && !floating {
            // Asleep, as long as something still holds it up.
            let mut below = self.aabb();
            below.translate(Vec3::Y * -0.01);
            if below.intersects(|c| world.solid(c)) {
                return false;
            }
            self.on_ground = false;
        }
        if floating {
            // Rises towards the surface and drifts with the current.
            let lift = if depth > 0.5 * SIZE { BUOYANCY } else { GRAVITY };
            self.velocity.y += (lift - GRAVITY) * dt;
            let flow = Vec3::new(current.x, 0.0, current.y);
            self.velocity = flow + (self.velocity - flow) * (-WATER_DRAG * dt).exp();
        } else {
            self.velocity.y -= GRAVITY * dt;
            let drag = if self.on_ground { GROUND_DRAG } else { AIR_DRAG };
            let keep = (-drag * dt).exp();
            self.velocity.x *= keep;
            self.velocity.z *= keep;
            self.velocity.y *= (-AIR_DRAG * dt).exp();
        }

        let before = self.position;
        let falling = self.velocity.y;
        let mut aabb = self.aabb();
        let blocked = move_and_slide(&mut aabb, self.velocity * dt, |p| world.solid(p));
        self.on_ground = blocked[1] && falling <= 0.0;
        for axis in [0, 2] {
            if blocked[axis] {
                self.velocity[axis] = 0.0;
            }
        }
        if blocked[1] {
            // A hard landing bounces, a soft one settles.
            self.velocity.y = if falling < -SETTLE { -falling * BOUNCE } else { 0.0 };
            self.on_ground &= self.velocity.y == 0.0;
        }
        if self.on_ground && self.velocity.with_y(0.0).length_squared() < REST * REST {
            self.velocity = Vec3::ZERO;
        }
        self.position = Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0);
        self.position != before
    }
}

/// Where the drops of the block at `cell` start, and how they pop out, from three random
/// numbers in 0..1: near the middle of the block, thrown up and a little sideways
/// (Minecraft's 0.2 up and up to 0.1 across a tick).
pub fn popped(cell: IVec3, random: [f32; 3]) -> Loose {
    let [a, b, c] = random;
    let offset = Vec3::new(a - 0.5, 0.0, b - 0.5) * 0.5;
    let angle = c * std::f32::consts::TAU;
    let velocity = Vec3::new(angle.cos() * 1.5, 4.0, angle.sin() * 1.5);
    Loose::new(cell.as_vec3() + Vec3::new(0.5, 0.25, 0.5) + offset, velocity)
}

/// The furthest across a broken block's drop comes to rest from the middle of its cell, on
/// flat open ground: from where it pops out, every way it may be thrown, stepped at the
/// server's 20 ticks a second.
pub fn pop_reach() -> f32 {
    struct Floor;
    impl Surroundings for Floor {
        fn solid(&self, cell: IVec3) -> bool {
            cell.y < 0
        }
        fn water(&self, _: &Aabb) -> (f32, glam::Vec2) {
            (0.0, glam::Vec2::ZERO)
        }
    }
    let mut furthest: f32 = 0.0;
    for (a, b) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.5, 0.0), (0.0, 0.5), (1.0, 0.5), (0.5, 1.0)] {
        for turn in 0..64 {
            let mut item = popped(IVec3::ZERO, [a, b, turn as f32 / 64.0]);
            for _ in 0..200 {
                item.step(&Floor, 0.05);
            }
            furthest = furthest.max(Vec3::new(item.position.x - 0.5, 0.0, item.position.z - 0.5).length());
        }
    }
    furthest
}

/// A stack thrown from `eye` towards `look`, as Minecraft throws one: from just below the
/// eye, at [`THROW_SPEED`], with a little lift.
pub fn thrown(eye: Vec3, look: Vec3) -> Loose {
    let look = look.normalize_or(Vec3::NEG_Z);
    Loose::new(eye - Vec3::Y * 0.3 - Vec3::Y * SIZE / 2.0, look * THROW_SPEED + Vec3::Y * 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    struct Scene<F: Fn(IVec3) -> bool> {
        solid: F,
        water: Option<f32>,
    }

    impl<F: Fn(IVec3) -> bool> Surroundings for Scene<F> {
        fn solid(&self, cell: IVec3) -> bool {
            (self.solid)(cell)
        }

        fn water(&self, aabb: &Aabb) -> (f32, Vec2) {
            (self.water.map_or(0.0, |surface| (surface - aabb.min.y).max(0.0)), Vec2::ZERO)
        }
    }

    /// A player takes an item touching their box grown by `REACH`, draws in one within
    /// `MAGNET` of their middle, and leaves one beyond both, however near `CLAIM` it lies.
    #[test]
    fn a_player_takes_what_touches_their_reach_and_draws_in_what_lies_within_the_magnet() {
        let feet = Vec3::new(0.5, 0.0, 0.5);
        let half = player::WIDTH / 2.0;
        let at = |dx: f32, y: f32| Loose::new(feet + Vec3::new(dx, y, 0.0), Vec3::ZERO);
        // The box's edge across, at the floor: the item's near side at it.
        let edge = half + REACH.x + SIZE / 2.0;
        assert_eq!(at(edge - 0.01, 0.0).pull(feet), Pull::Taken);
        assert_eq!(at(edge + 0.01, 0.0).pull(feet), Pull::Drawn);
        // Over the head: the item's bottom at the top of the reach.
        assert_eq!(at(0.0, player::HEIGHT + REACH.y - 0.01).pull(feet), Pull::Taken);
        assert_eq!(at(0.0, player::HEIGHT + REACH.y + 0.01).pull(feet), Pull::Drawn);
        // The magnet's edge, level with the middle.
        let level = player::HEIGHT * 0.5 - SIZE / 2.0;
        assert_eq!(at(MAGNET - 0.01, level).pull(feet), Pull::Drawn);
        assert_eq!(at(MAGNET + 0.01, level).pull(feet), Pull::Left);
        assert_eq!(at(CLAIM - 0.01, level).pull(feet), Pull::Left);
    }

    fn run(item: &mut Loose, world: &impl Surroundings, seconds: f32) {
        for _ in 0..(seconds * 20.0) as usize {
            item.step(world, 0.05);
        }
    }

    /// A player standing on a broken block's cell draws in its drop wherever it popped to:
    /// the furthest it comes to rest, from their middle, stays inside `MAGNET` (the gate's
    /// furnishings come back so).
    #[test]
    fn a_drop_comes_to_rest_within_reach_of_one_standing_where_it_broke() {
        let reach = pop_reach();
        assert!(reach > 0.5, "drops hardly pop out: {reach}");
        let from_middle = Vec3::new(reach, crate::physics::player::HEIGHT / 2.0 - SIZE / 2.0, 0.0).length();
        assert!(from_middle < MAGNET, "a drop rests {from_middle:.2} blocks from the middle of one standing where it broke, past MAGNET {MAGNET}");
    }

    #[test]
    fn popped_drops_hop_out_and_come_to_rest_on_the_ground() {
        let floor = Scene { solid: |c: IVec3| c.y < 0, water: None };
        let mut item = popped(IVec3::new(0, 0, 0), [0.5, 0.5, 0.25]);
        let mut highest = item.position.y;
        for _ in 0..20 {
            item.step(&floor, 0.05);
            highest = highest.max(item.position.y);
        }
        assert!(highest > 0.5, "no hop: {highest}");
        run(&mut item, &floor, 3.0);
        assert!(item.resting(), "{item:?}");
        assert!(item.position.y.abs() < 1e-4);
        // Asleep: another step changes nothing.
        assert!(!item.step(&floor, 0.05));
    }

    #[test]
    fn items_fall_when_the_ground_goes() {
        let mut item = Loose { position: Vec3::new(0.5, 0.0, 0.5), velocity: Vec3::ZERO, on_ground: true };
        let gone = Scene { solid: |c: IVec3| c.y < -3, water: None };
        run(&mut item, &gone, 2.0);
        assert!((item.position.y + 3.0).abs() < 1e-4, "{item:?}");
    }

    #[test]
    fn items_float_up_to_the_surface() {
        let pond = Scene { solid: |c: IVec3| c.y < -4, water: Some(0.0) };
        let mut item = Loose::new(Vec3::new(0.5, -3.5, 0.5), Vec3::ZERO);
        run(&mut item, &pond, 6.0);
        assert!(item.position.y > -SIZE && item.position.y < 0.1, "{item:?}");
    }

    #[test]
    fn thrown_stacks_fly_forward_and_stop_at_walls() {
        let floor = Scene { solid: |c: IVec3| c.y < 0 || c.x >= 3, water: None };
        let mut item = thrown(Vec3::new(0.5, 1.62, 0.5), Vec3::X);
        run(&mut item, &floor, 3.0);
        assert!(item.position.x > 2.0 && item.position.x <= 3.0 - SIZE / 2.0 + 1e-3, "{item:?}");
        assert!(item.resting());
    }
}
