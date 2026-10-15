//! How a creature's body moves: walking toward what its brain wants, turning at its turn
//! rate, jumping up single steps, falling, floating in water, drifting in the air for
//! creatures that float, and flying for fliers: on the wing whenever they go faster than a
//! walk, keeping their height over the land and what grows on it, and fluttering down to
//! land when they slow. The same box collision as players (`physics::move_and_slide`),
//! sized to the creature.

use glam::{IVec3, Vec3};

use super::{Movement, Style, Water};
use crate::physics::{Aabb, move_and_slide};

/// Blocks per second squared, as players fall.
const GRAVITY: f32 = 32.0;
const TERMINAL: f32 = 60.0;
/// Height above the ground a drifting creature floats at.
const DRIFT_HEIGHT: f32 = 1.6;
/// A flier takes wing when asked to go faster than this many times its walk.
pub const TAKES_WING: f32 = 1.5;
/// How fast a flier climbs and comes down in flight, and flutters down to land, in blocks
/// per second; and how far ahead it looks for rising land.
const CLIMB: f32 = 5.0;
const DESCEND: f32 = 3.0;
const FLUTTER_DOWN: f32 = 2.5;
const LOOK_AHEAD: f32 = 3.0;

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
    /// Which way round it turns, 1 or -1, once its head barred the short way (round a wall
    /// at its side); 0 turns the short way. Kept only while it still wants the heading it
    /// took the long way for (`turn_to`, a yaw): wanting another, it turns the short way
    /// again rather than spin round.
    pub turn_way: f32,
    pub turn_to: f32,
}

impl Body {
    pub fn new(position: Vec3, yaw: f32, size: (f32, f32)) -> Self {
        Self { position, velocity: Vec3::ZERO, yaw, on_ground: false, in_water: false, size, against_wall: false, reach: 0.0, head: 0.0, turn_way: 0.0, turn_to: 0.0 }
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

/// Whether a flier is on the wing with this drive: going somewhere faster than a walk.
pub fn flies(movement: &Movement, drive: Drive) -> bool {
    drive.wish != Vec3::ZERO && drive.speed > movement.walk * TAKES_WING
}

/// How far round a creature turns, in degrees, to find a way past what shuts the one it
/// wants: a little each side first, then further, at last back past where it came from.
const DETOURS: [f32; 4] = [40.0, 80.0, 120.0, 160.0];

/// The deepest drop, in blocks, a walker steps down of its own accord: it goes round the
/// edge of anything higher, as it does a wall.
pub const SAFE_DROP: i32 = 3;

/// How high a jump at `jump` blocks per second lifts it, stepped as `dt` steps it.
fn hop_height(jump: f32, dt: f32) -> f32 {
    let (mut v, mut up) = (jump, 0.0);
    while v > 0.0 {
        up += v * dt;
        v -= GRAVITY * dt;
    }
    up
}

/// The least jump that hops up a single block, stepped as `dt` steps it: anything that
/// jumps at all gets up one, however low its own jump (its jump is for leaps and hops).
fn climb_speed(dt: f32) -> f32 {
    let (mut low, mut high) = (0.0, 30.0);
    for _ in 0..24 {
        let mid = (low + high) / 2.0;
        if hop_height(mid, dt) >= 1.1 { high = mid } else { low = mid }
    }
    high
}

/// Whether the body can go a little way along `heading` (horizontal, unit length): sliding
/// along a wall counts, and so does a single step up for anything that jumps; its head must
/// not meet rock there, and on the ground it must not step off a drop deeper than
/// `SAFE_DROP` or, a land animal on dry land, into water two deep.
pub fn open_way(body: &Body, world: &impl Surroundings, movement: &Movement, water: Water, heading: Vec3) -> bool {
    let probe = body.size.0 * 0.5 + 0.3;
    let solid = |p: IVec3| world.solid(p);
    let goes = |lift: f32| {
        let mut aabb = body.aabb();
        aabb.translate(Vec3::Y * lift);
        if lift > 0.0 && aabb.intersects(solid) {
            return None;
        }
        let start = aabb.min;
        move_and_slide(&mut aabb, heading * probe, solid);
        let gone = (aabb.min - start).dot(heading);
        (gone >= probe * 0.5).then(|| Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0))
    };
    let Some(to) = goes(0.0).or_else(|| if movement.jump > 0.0 { goes(1.05) } else { None }) else { return false };
    let ahead = Body { position: to, yaw: yaw_of(heading), ..*body };
    if ahead.head_blocked(world) {
        return false;
    }
    // What it would come down on along its way, from here to as far ahead as it carries
    // on before it can turn (a running deer, or a hare mid-hop, goes a block or so),
    // against what it stands on now: the drop between them, and water there, judged the
    // same on the ground or in the air. A wall along the way is ground to no one.
    let reach = SAFE_DROP + 4;
    let landing = |from: IVec3| (0..=reach).map(|k| from - IVec3::Y * k).find(|&c| world.water(c) || world.solid(c - IVec3::Y));
    // What it stands on is the highest ground under any part of its box: at an edge, its
    // middle may already be over the drop.
    let half = body.size.0 * 0.5 - 0.05;
    let Some(now) = [(0.0, 0.0), (half, half), (half, -half), (-half, half), (-half, -half)]
        .into_iter()
        .filter_map(|(x, z)| landing((body.position + Vec3::new(x, 0.01, z)).floor().as_ivec3()))
        .map(|c| c.y)
        .max()
    else {
        // Already over a drop deeper than it looks: nothing ahead is worse.
        return true;
    };
    let ahead = body.size.0 + body.velocity.with_y(0.0).length() * 0.3;
    // A land animal shies from deep water (two deep) unless it is already out of its depth:
    // wading in the shallows, it does not walk on into the deep.
    let feet = body.position.floor().as_ivec3();
    let wary = water == Water::Land && !(body.in_water && world.water(feet - IVec3::Y));
    // Along the way it really goes, sliding along a wall as its box would, a step at a
    // time; ahead of its middle by half its width.
    let samples = (ahead / 0.4).ceil().max(1.0) as i32;
    let step = heading * (ahead / samples as f32);
    let mut aabb = Aabb::standing(to, body.size.0, body.size.1);
    (0..=samples).all(|i| {
        if i > 0 {
            move_and_slide(&mut aabb, step, solid);
        }
        let middle = Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0);
        let cell = (middle + Vec3::Y * 0.01).floor().as_ivec3();
        if world.solid(cell) {
            return true;
        }
        match landing(cell) {
            None => false,
            Some(then) => now - then.y <= SAFE_DROP && !(wary && world.water(then) && world.water(then - IVec3::Y)),
        }
    })
}

/// The way a walking body goes for a `wish` (horizontal, unit length): the wish itself
/// while it is open (`open_way`), else the nearest turn from it that is, to the side it
/// already took (`side`, 1 or -1; 0 for none yet: the side it faces more). Returns the
/// heading and the side taken, 0 when it goes straight; boxed in, it stands (no heading,
/// and 0) rather than press into a wall or step off a cliff. This is
/// how anything walking gets round a wall, a corner or a tree rather than pressing into it.
pub fn steer(body: &Body, world: &impl Surroundings, movement: &Movement, water: Water, wish: Vec3, side: f32) -> (Vec3, f32) {
    if wish == Vec3::ZERO || open_way(body, world, movement, water, wish) {
        return (wish, 0.0);
    }
    let turned = |sign: f32, degrees: f32| glam::Quat::from_rotation_y(sign * degrees.to_radians()) * wish;
    let first = if side != 0.0 {
        side.signum()
    } else if turned(1.0, 90.0).dot(body.forward()) >= turned(-1.0, 90.0).dot(body.forward()) {
        1.0
    } else {
        -1.0
    };
    // Going round already, it keeps to its side all the way round before it tries the
    // other, so it does not dither between two ways out; starting, the nearest turn either
    // side wins.
    let tries: Vec<(f32, f32)> = if side != 0.0 {
        [first, -first].into_iter().flat_map(|sign| DETOURS.map(|degrees| (sign, degrees))).collect()
    } else {
        DETOURS.into_iter().flat_map(|degrees| [(first, degrees), (-first, degrees)]).collect()
    };
    tries.into_iter().map(|(sign, degrees)| (turned(sign, degrees), sign)).find(|&(heading, _)| open_way(body, world, movement, water, heading)).unwrap_or((Vec3::ZERO, 0.0))
}

/// Whether a body moving with this drive walks on the land, where `steer` finds its way:
/// not on the wing, drifting, floating or a swimmer.
pub fn walks(movement: &Movement, water: Water, drive: Drive) -> bool {
    !(movement.style == Style::Flier && flies(movement, drive)) && movement.style != Style::Drifter && !matches!(water, Water::Air | Water::Swim)
}

/// How far apart two yaws are, the short way round, in radians.
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d)
}

/// Advances the body by `dt` seconds.
pub fn step(body: &mut Body, world: &impl Surroundings, movement: &Movement, water: Water, drive: Drive, dt: f32) {
    let feet = body.position.floor().as_ivec3();
    let middle = (body.position + Vec3::Y * body.size.1 * 0.5).floor().as_ivec3();
    body.in_water = world.water(feet) || world.water(middle);
    let flier = movement.style == Style::Flier;
    let flying = flier && flies(movement, drive);
    let floating = water == Water::Air || movement.style == Style::Drifter;

    let (start, start_yaw) = (body.position, body.yaw);
    // A head already in something (a block set before its face) is not held there: it may
    // turn and walk out, or it would stand frozen for good.
    let head_was_blocked = body.head_blocked(world);
    // Face the way it moves, or what it was asked to look at.
    let moving = drive.wish.length_squared() > 1e-4;
    let heading = if moving { Some(drive.wish) } else { drive.face };
    let wanted = heading.filter(|h| h.x.abs() + h.z.abs() > 1e-4).map(yaw_of);
    if let Some(to) = wanted {
        let most = movement.turn.to_radians() * dt;
        if body.turn_way != 0.0 && angle_between(to, body.turn_to) > 45f32.to_radians() {
            body.turn_way = 0.0;
        }
        if body.turn_way == 0.0 {
            body.yaw = turn(body.yaw, to, most);
        } else {
            let left = ((to - body.yaw) * body.turn_way).rem_euclid(std::f32::consts::TAU);
            body.yaw += body.turn_way * left.min(most);
            if left <= most {
                body.turn_way = 0.0;
            }
        }
    }

    // Walk in the direction it faces once it roughly faces it, so it turns before it goes.
    let facing = body.forward();
    let along = drive.wish.with_y(0.0).normalize_or_zero();
    let aligned = along.dot(facing).max(0.0);
    let target = along * drive.speed * (0.3 + 0.7 * aligned);
    let control = if body.on_ground || body.in_water || floating || flying { 10.0 } else if flier { 6.0 } else { 2.0 };
    let horizontal = body.velocity.with_y(0.0).lerp(target.with_y(0.0), (control * dt).min(1.0));
    body.velocity = horizontal.with_y(body.velocity.y);

    if flying {
        // Keep its height over the land here and just ahead, so it rises over a hill or a
        // tree before it reaches it.
        let ahead = (body.position + facing * LOOK_AHEAD).floor().as_ivec3();
        let top = |column: IVec3, from: i32| (0..from + 24).map(|d| column + IVec3::Y * (from - d)).find(|&c| world.solid(c) || world.water(c)).map(|c| c.y as f32 + 1.0);
        let reach = movement.fly.ceil() as i32 + 2;
        let ground = top(feet, 0).into_iter().chain(top(ahead, reach)).fold(f32::MIN, f32::max);
        let want = if ground == f32::MIN { body.position.y } else { ground + movement.fly };
        body.velocity.y = ((want - body.position.y) * 1.5).clamp(-DESCEND, CLIMB);
    } else if flier && !body.on_ground && !body.in_water {
        // Flutters down to land.
        body.velocity.y = (body.velocity.y - GRAVITY * dt).max(-FLUTTER_DOWN);
    } else if floating {
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

    // Hop up a step it walks into, if it can jump and there is room above: a block before
    // its feet, not only its head stopped at a door frame. Paddling at a bank, anything
    // that walks hauls itself out the same way.
    let paddling = body.in_water && water != Water::Swim && !floating && !flying;
    // Rising from a hop its head's step began: the head goes on over the step, not stopped
    // at it, or the body would bounce at the foot of the step for ever.
    let mut head_hop = false;
    if (body.on_ground && movement.jump > 0.0 || paddling) && body.against_wall && along != Vec3::ZERO {
        let (mut level, mut raised) = (body.aabb(), body.aabb());
        level.translate(along * 0.3);
        raised.translate(Vec3::Y * 1.05 + along * 0.3);
        let boxed = level.intersects(|p| world.solid(p));
        // A long head (a boar's snout) stops the body short of a step its box never meets.
        // Judged at the feet, as the box's step is: the cell before the head, at the feet's
        // level, is solid with room above it for the whole body, and the body raised a
        // block clears it once over the step. A wall, a lintel at head height or a step
        // under a low ceiling is no step. Only charges and tracking drive a body straight
        // at such a step: steering (`open_way`) still goes round it.
        let half = body.size.0 * 0.5;
        let head_step = !boxed && body.reach > half + 0.05 && along.dot(body.forward()) > 0.7 && {
            let ahead = body.position + along * (body.reach + 0.3);
            let cell = IVec3::new(ahead.x.floor() as i32, (body.position.y + 0.01).floor() as i32, ahead.z.floor() as i32);
            let mut over = body.aabb();
            over.translate(Vec3::Y * 1.05 + along * (body.reach - half + 0.3));
            world.solid(cell) && (1..=body.size.1.ceil() as i32).all(|up| !world.solid(cell + IVec3::Y * up)) && !over.intersects(|p| world.solid(p))
        };
        if (boxed && !raised.intersects(|p| world.solid(p))) || head_step {
            body.velocity.y = movement.jump.max(climb_speed(dt));
            head_hop = head_step;
        }
    }
    // Hoppers move in hops whenever they move, and so do fliers on the ground: each hop
    // springs the way it wants to go, not the way the last one carried it (it can hardly
    // turn in the air, so a hop still carrying it at a cliff edge would go over).
    if matches!(movement.style, Style::Hopper | Style::Flier) && body.on_ground && target.length_squared() > 0.05 && body.velocity.y <= 0.0 {
        body.velocity = target.with_y(movement.jump.min(6.0) * 0.75);
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
    // Its head stops at what its box does not. Keeping the step, it turns the other way
    // round if that is clear (once: then it keeps to that way round), or, stepping on the
    // way it faces, holds its facing (edging
    // along a wall); else it undoes the step and keeps the turn, or keeps the turn and
    // backs from what its head meets, as an animal turning in a corner backs as it turns.
    // Where nothing is clear (a long head turning in a doorway) it turns where it stood,
    // its head brushing the frame for a moment, rather than spin or walk on backwards. A
    // step undone counts as against the wall, so it climbs or turns away.
    if !head_was_blocked && !head_hop && body.head_blocked(world) {
        let (moved, turned) = (body.position, body.yaw);
        let fits = |at: Vec3| !Aabb::standing(at, body.size.0, body.size.1).intersects(|p| world.solid(p));
        let clear = |at: Vec3, yaw: f32| fits(at) && !Body { position: at, yaw, ..*body }.head_blocked(world);
        let back = Vec3::new(start.x, moved.y, start.z);
        let other_way = start_yaw - (turned - start_yaw);
        let onward = (moved - start).with_y(0.0).dot(Body { yaw: start_yaw, ..*body }.forward()) >= -1e-4;
        // Standing to face something, the long way round only to a heading its head fits
        // at: one it cannot face (a player behind a wall it stands against) it would turn
        // for for ever.
        let faceable = moving || wanted.is_none_or(|to| clear(moved, to));
        if turned != start_yaw && body.turn_way == 0.0 && clear(moved, other_way) && faceable {
            body.yaw = other_way;
            body.turn_way = (other_way - start_yaw).signum();
            body.turn_to = wanted.unwrap_or(turned);
        } else if onward && clear(moved, start_yaw) {
            body.yaw = start_yaw;
        } else {
            if clear(back, turned) {
                body.position = back;
            } else if let Some(at) = (1..=4).map(|k| moved - body.forward() * (0.12 * k as f32)).find(|&at| clear(at, turned)) {
                body.position = at;
            } else if turned != start_yaw && fits(back) {
                // Committed to its way round, it keeps turning.
                body.position = back;
            } else {
                body.yaw = start_yaw;
                if fits(back) {
                    body.position = back;
                }
            }
            body.velocity.x = 0.0;
            body.velocity.z = 0.0;
            body.against_wall = true;
        }
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

    /// A wall `high` blocks tall from x = 3 on, over flat ground.
    struct Ledge {
        high: i32,
    }

    impl Surroundings for Ledge {
        fn solid(&self, cell: IVec3) -> bool {
            cell.y < 0 || (cell.x >= 3 && cell.y < self.high)
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    #[test]
    fn walkers_steer_along_a_wall_they_cannot_climb_and_keep_to_their_side() {
        let wall = Ledge { high: 3 };
        let movement = Movement::default();
        // Flush against the wall, facing a little toward +z.
        let mut body = Body::new(Vec3::new(2.7, 0.0, 0.5), yaw_of(Vec3::new(0.3, 0.0, 1.0)), (0.6, 0.8));
        let (heading, side) = steer(&body, &wall, &movement, Water::Land, Vec3::X, 0.0);
        assert!(side != 0.0 && heading.dot(Vec3::X).abs() < 0.5, "it pressed on into the wall: {heading}");
        assert!(heading.z > 0.0, "it went round the way it faced less: {heading}");
        // Going round, it keeps to its side though the other is as open.
        let (again, kept) = steer(&body, &wall, &movement, Water::Land, Vec3::X, -side);
        assert_eq!(kept, -side, "it changed sides going round");
        assert!(again.z < 0.0);
        // An open way is taken as it is.
        assert_eq!(steer(&body, &wall, &movement, Water::Land, -Vec3::X, side), (-Vec3::X, 0.0));
        // Walking it, it slides along and is never pressed still.
        for _ in 0..40 {
            let (heading, _) = steer(&body, &wall, &movement, Water::Land, Vec3::X, side);
            step(&mut body, &wall, &movement, Water::Land, Drive { wish: heading, speed: 2.0, face: None }, 0.05);
        }
        assert!(body.position.z > 2.5, "it barely moved along the wall: {}", body.position);
        assert!(body.position.x + 0.3 <= 3.0 + 1e-3, "it went into the wall");
    }

    #[test]
    fn anything_that_jumps_gets_up_a_single_block_and_no_higher() {
        let step_up = Ledge { high: 1 };
        let body = Body::new(Vec3::new(2.7, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        let (low, none) = (Movement { jump: 6.0, ..Movement::default() }, Movement { jump: 0.0, ..Movement::default() });
        assert!(hop_height(6.0, 0.05) < 1.0 && hop_height(climb_speed(0.05), 0.05) >= 1.1);
        assert!(open_way(&body, &step_up, &low, Water::Land, Vec3::X), "a low jumper found a single block a wall");
        assert!(!open_way(&body, &step_up, &none, Water::Land, Vec3::X), "a block was open to one that cannot jump");
        assert!(!open_way(&body, &Ledge { high: 2 }, &low, Water::Land, Vec3::X), "a wall two high was open");
        // And it really gets up, though its own jump would not lift it a block.
        let mut walker = Body::new(Vec3::new(1.5, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        walker.on_ground = true;
        for _ in 0..60 {
            step(&mut walker, &step_up, &low, Water::Land, Drive { wish: Vec3::X, speed: 2.0, face: None }, 0.05);
        }
        assert!(walker.position.y >= 1.0 && walker.position.x > 3.5, "it never got up the step: {}", walker.position);
    }

    /// A long head meets a step before the body does, and stops it there (its head stops at
    /// what its box does not): it hops the step all the same, as a short-headed walker does.
    #[test]
    fn a_long_headed_walker_hops_a_single_block_its_snout_meets_first() {
        let step_up = Ledge { high: 1 };
        let movement = Movement { jump: 6.0, ..Movement::default() };
        let mut boar = Body::new(Vec3::new(0.5, 0.0, 0.5), yaw_of(Vec3::X), (1.2, 1.2));
        (boar.on_ground, boar.reach, boar.head) = (true, 1.6, 0.9);
        assert!(!Body { position: Vec3::new(1.6, 0.0, 0.5), ..boar }.aabb().intersects(|p| step_up.solid(p)), "the box clears the step where the head meets it");
        assert!(Body { position: Vec3::new(1.6, 0.0, 0.5), ..boar }.head_blocked(&step_up), "the head meets the step first");
        for _ in 0..80 {
            step(&mut boar, &step_up, &movement, Water::Land, Drive { wish: Vec3::X, speed: 4.0, face: None }, 0.05);
        }
        assert!(boar.position.y >= 1.0 && boar.position.x > 3.5, "its snout held it at the step: {}", boar.position);
    }

    /// A low snout (a badger's) meets a step its body does not reach either: driven straight
    /// at it, it gets up, not bouncing at its foot.
    #[test]
    fn a_low_snout_driven_at_a_step_gets_up_it() {
        let step_up = Ledge { high: 1 };
        let movement = Movement { jump: 6.0, ..Movement::default() };
        let mut badger = Body::new(Vec3::new(0.5, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.5));
        (badger.on_ground, badger.reach, badger.head) = (true, 0.7, 0.42);
        for _ in 0..80 {
            step(&mut badger, &step_up, &movement, Water::Land, Drive { wish: Vec3::X, speed: 3.0, face: None }, 0.05);
        }
        assert!(badger.position.y >= 1.0 && badger.position.x > 3.3, "it never got up the step: {}", badger.position);
    }

    /// What its head meets is judged at its feet: a tall head over a wall two high, or under
    /// a beam at head height with open ground beneath, finds no step and never hops.
    #[test]
    fn a_tall_head_finds_no_step_in_a_wall_two_high_or_a_beam() {
        struct Beam;
        impl Surroundings for Beam {
            fn solid(&self, cell: IVec3) -> bool {
                cell.y < 0 || (cell.x >= 3 && cell.y == 1)
            }
            fn water(&self, _: IVec3) -> bool {
                false
            }
        }
        let movement = Movement { jump: 12.0, ..Movement::default() };
        let stag = || {
            let mut stag = Body::new(Vec3::new(0.5, 0.0, 0.5), yaw_of(Vec3::X), (0.8, 1.6));
            (stag.on_ground, stag.reach, stag.head) = (true, 1.2, 1.55);
            stag
        };
        let run = |world: &dyn Fn(&mut Body)| {
            let mut stag = stag();
            let mut hops = 0;
            for _ in 0..200 {
                let was = stag.velocity.y;
                world(&mut stag);
                if stag.velocity.y > 0.0 && was <= 0.0 {
                    hops += 1;
                }
            }
            (hops, stag.position)
        };
        let wall = Ledge { high: 2 };
        let (hops, at) = run(&|stag| step(stag, &wall, &movement, Water::Land, Drive { wish: Vec3::X, speed: 4.0, face: None }, 0.05));
        assert!(hops == 0 && at.y < 0.5 && at.x < 3.0, "a wall two high: {hops} hops, at {at}");
        let (hops, at) = run(&|stag| step(stag, &Beam, &movement, Water::Land, Drive { wish: Vec3::X, speed: 4.0, face: None }, 0.05));
        assert!(hops == 0 && at.y < 0.5, "a beam at head height: {hops} hops, at {at}");
    }

    #[test]
    fn walkers_go_round_a_cliff_edge_and_deep_water_but_step_down_a_little() {
        // Flat ground to x = 3, then a drop of `drop` blocks, water-filled to `wet` below
        // the rim.
        struct Edge {
            drop: i32,
            wet: i32,
        }
        impl Surroundings for Edge {
            fn solid(&self, cell: IVec3) -> bool {
                if cell.x < 3 { cell.y < 0 } else { cell.y < -self.drop }
            }
            fn water(&self, cell: IVec3) -> bool {
                cell.x >= 3 && cell.y < 0 && cell.y >= -self.wet && cell.y >= -self.drop
            }
        }
        let movement = Movement::default();
        let mut body = Body::new(Vec3::new(2.6, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        body.on_ground = true;
        let open = |edge: &Edge, water: Water| open_way(&body, edge, &movement, water, Vec3::X);
        assert!(open(&Edge { drop: SAFE_DROP, wet: 0 }, Water::Land), "a short drop was a wall");
        assert!(!open(&Edge { drop: SAFE_DROP + 3, wet: 0 }, Water::Land), "it stepped off a cliff");
        assert!(open(&Edge { drop: 1, wet: 1 }, Water::Land), "a puddle was a wall");
        assert!(!open(&Edge { drop: 3, wet: 3 }, Water::Land), "a land animal walked into deep water");
        assert!(open(&Edge { drop: 3, wet: 3 }, Water::Amphibious), "a duck kept out of the pond");
        // Wading in the shallows, it keeps out of the deep as on the bank; out of its depth,
        // nothing is worse ahead.
        struct Shelf;
        impl Surroundings for Shelf {
            fn solid(&self, cell: IVec3) -> bool {
                cell.y < if cell.x < 3 { -1 } else { -3 }
            }
            fn water(&self, cell: IVec3) -> bool {
                (-3..0).contains(&cell.y) && !self.solid(cell)
            }
        }
        let mut wading = Body::new(Vec3::new(2.6, -1.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        (wading.on_ground, wading.in_water) = (true, true);
        assert!(!open_way(&wading, &Shelf, &movement, Water::Land, Vec3::X), "wading, it walked on into deep water");
        let mut swimming = Body::new(Vec3::new(3.5, -2.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        swimming.in_water = true;
        assert!(open_way(&swimming, &Shelf, &movement, Water::Land, Vec3::X), "out of its depth, deeper water was a wall");
        // Steered at the edge, it goes along it; boxed in by cliffs, it stands.
        let cliff = Edge { drop: 8, wet: 0 };
        let (heading, side) = steer(&body, &cliff, &movement, Water::Land, Vec3::X, 0.0);
        assert!(side != 0.0 && heading.x < 0.5, "it went over the edge: {heading}");
        for _ in 0..60 {
            let (heading, _) = steer(&body, &cliff, &movement, Water::Land, Vec3::X, side);
            step(&mut body, &cliff, &movement, Water::Land, Drive { wish: heading, speed: 4.0, face: None }, 0.05);
            assert!(body.position.y > -0.5, "it fell off the cliff at {}", body.position);
        }
    }

    #[test]
    fn a_head_in_a_block_set_before_it_turns_out_and_a_turn_toward_a_wall_goes_the_other_way() {
        // A block before its face: it is not held there, it turns away.
        let wall = Ledge { high: 3 };
        let movement = Movement { jump: 0.0, ..Movement::default() };
        let mut body = Body::new(Vec3::new(2.2, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 1.4));
        body.reach = 0.9;
        body.head = 1.4;
        assert!(body.head_blocked(&wall));
        for _ in 0..30 {
            step(&mut body, &wall, &movement, Water::Land, Drive { wish: -Vec3::X, speed: 2.0, face: None }, 0.05);
        }
        assert!(!body.head_blocked(&wall) && body.position.x < 1.5, "it stayed with its head in the block at {}", body.position);
        // Turning about from facing along the wall, the short way would sweep its head into
        // the wall: it turns the other way round, and its head never goes in.
        let mut body = Body::new(Vec3::new(2.6, 0.0, 0.5), yaw_of(-Vec3::Z), (0.6, 1.4));
        body.reach = 0.9;
        body.head = 1.4;
        for _ in 0..40 {
            step(&mut body, &wall, &movement, Water::Land, Drive { wish: Vec3::new(0.2, 0.0, 1.0).normalize(), speed: 2.0, face: None }, 0.05);
            assert!(!body.head_blocked(&wall), "its head went into the wall at {}", body.position);
        }
        assert!(body.forward().z > 0.9, "it never turned: facing {}", body.forward());
        // Wanting another way part way round the long turn, it turns the short way to it
        // rather than finish a spin it no longer wants.
        let mut body = Body::new(Vec3::new(2.6, 0.0, 0.5), yaw_of(-Vec3::Z), (0.6, 1.4));
        body.reach = 0.9;
        body.head = 1.4;
        for _ in 0..3 {
            step(&mut body, &wall, &movement, Water::Land, Drive { wish: Vec3::new(0.2, 0.0, 1.0).normalize(), speed: 2.0, face: None }, 0.05);
        }
        assert!(body.turn_way != 0.0, "it never took the long way");
        let from = body.yaw;
        step(&mut body, &wall, &movement, Water::Land, Drive { wish: -Vec3::X, speed: 0.0, face: None }, 0.05);
        assert_eq!(body.turn_way, 0.0, "the long turn outlived the heading it was for");
        assert!(angle_between(body.yaw, yaw_of(-Vec3::X)) < angle_between(from, yaw_of(-Vec3::X)), "it turned away from the new heading");
        // Standing to look at something through the wall, which its head cannot face, it
        // turns as far as it can and does not spin round for it.
        let mut body = Body::new(Vec3::new(2.6, 0.0, 0.5), yaw_of(-Vec3::Z), (0.6, 1.4));
        body.reach = 0.9;
        body.head = 1.4;
        let mut swept = 0.0;
        for _ in 0..200 {
            let was = body.yaw;
            step(&mut body, &wall, &movement, Water::Land, Drive { wish: Vec3::ZERO, speed: 0.0, face: Some(Vec3::new(1.0, 0.0, 0.05)) }, 0.05);
            swept += angle_between(body.yaw, was);
            assert!(!body.head_blocked(&wall), "its head went into the wall facing {}", body.forward());
        }
        assert!(swept < std::f32::consts::PI, "it spun {swept} radians looking at what it cannot face");
    }

    #[test]
    fn fliers_take_wing_clear_what_rises_and_flutter_down_to_land() {
        // Flat ground, then a wall five blocks high from x = 12.
        struct Hill;
        impl Surroundings for Hill {
            fn solid(&self, cell: IVec3) -> bool {
                cell.y < 0 || (cell.x >= 12 && cell.y < 5)
            }
            fn water(&self, _: IVec3) -> bool {
                false
            }
        }
        let movement = Movement { style: Style::Flier, walk: 1.0, run: 6.0, jump: 3.0, fly: 4.0, ..Movement::default() };
        let mut body = Body::new(Vec3::new(0.5, 0.0, 0.5), -std::f32::consts::FRAC_PI_2, (0.2, 0.2));
        let go = |body: &mut Body, speed: f32, wish: Vec3, seconds: f32, lowest: &mut f32| {
            for _ in 0..(seconds * 20.0) as usize {
                step(body, &Hill, &movement, Water::Land, Drive { wish, speed, face: None }, 0.05);
                if body.position.x > 11.8 && body.position.x < 16.0 {
                    *lowest = lowest.min(body.position.y);
                }
            }
        };
        // At a walk it hops along the ground and never leaves it for long.
        let mut lowest = f32::MAX;
        go(&mut body, 1.0, Vec3::X, 2.0, &mut lowest);
        assert!(body.position.y < 1.0, "a walk took it up to {}", body.position.y);
        // Fast, it flies at its height and rises over the wall before reaching it.
        go(&mut body, 6.0, Vec3::X, 1.5, &mut lowest);
        assert!(body.position.y > 3.0 && !body.on_ground, "took wing only to {}", body.position.y);
        go(&mut body, 6.0, Vec3::X, 3.0, &mut lowest);
        assert!(body.position.x > 16.0, "stopped at the wall at {}", body.position.x);
        assert!(lowest >= 5.0, "flew into the wall at height {lowest}");
        assert!((body.position.y - 9.0).abs() < 1.0, "keeps 4 over the hill top: {}", body.position.y);
        // Stopped, it flutters down and lands, never falling hard.
        let mut fastest = 0.0f32;
        for _ in 0..80 {
            step(&mut body, &Hill, &movement, Water::Land, Drive::default(), 0.05);
            fastest = fastest.max(-body.velocity.y);
        }
        assert!(body.on_ground && (body.position.y - 5.0).abs() < 1e-3, "did not land: {}", body.position);
        assert!(fastest <= FLUTTER_DOWN + 1e-3, "fell at {fastest}");
    }

    #[test]
    fn steps_up_a_block() {
        let world = Flat { wall: Some(3) };
        let mut body = Body::new(Vec3::new(0.5, 0.0, 0.5), -std::f32::consts::FRAC_PI_2, (0.6, 0.8));
        walk(&mut body, &world, Vec3::X, 4.0);
        assert!(body.position.x > 3.5 && body.position.y >= 1.0 - 1e-3, "stuck at {}", body.position);
    }
}
