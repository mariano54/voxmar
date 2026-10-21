//! How a creature's body moves: walking toward what its brain wants, turning at its turn
//! rate, jumping up single steps, falling, floating in water, drifting in the air for
//! creatures that float, and flying for fliers: on the wing whenever they go faster than a
//! walk, keeping their height over the land and what grows on it, and fluttering down to
//! land when they slow. The same box collision as players (`physics::move_and_slide_boxes`,
//! against blocks' shapes: a slab's lower half, a post), sized to the creature; it walks up
//! a half step (`Movement::step`) without a hop, as a player does.

use glam::{IVec3, Vec3};

use super::path::{self, Walker};
use super::{Movement, Style, Water};
use crate::physics::{Aabb, CUBE, move_and_slide_boxes, move_axis_boxes};

/// Blocks per second squared, as players fall.
pub const GRAVITY: f32 = 32.0;
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
    /// How far it lifts its head over what it meets low down, 0 to `HEAD_LIFT`: up the next
    /// tread of a stair or the next course of a roof while its feet are on the last one.
    /// Its head (`head_points`) is that much higher; it lowers it again once it can.
    pub lift: f32,
    /// Whether it goes down where it could not climb back of its own accord: in a chase
    /// (the server's say), after what it chases. Otherwise it walks into no pit it could
    /// not get out of (`path::way_out`), however lively its pace.
    pub bold: bool,
}

impl Body {
    pub fn new(position: Vec3, yaw: f32, size: (f32, f32)) -> Self {
        Self { position, velocity: Vec3::ZERO, yaw, on_ground: false, in_water: false, size, against_wall: false, reach: 0.0, head: 0.0, turn_way: 0.0, turn_to: 0.0, lift: 0.0, bold: false }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, self.size.0, self.size.1)
    }

    /// The direction it faces.
    pub fn forward(&self) -> Vec3 {
        Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos())
    }

    /// Whether its head (and the neck on the way to it), beyond its box, is in something
    /// solid, lifted as it holds it (`lift`).
    pub fn head_blocked(&self, world: &impl Surroundings) -> bool {
        self.head_points().any(|p| inside(world, p))
    }

    /// Whether what its head meets going `along` (within a little of the way it faces), at
    /// `speed`, is a step it gets up with a hop of `jump`, judged at its feet as its box's
    /// step is: the first cell at its feet's level that it faces, out from its box's front
    /// past its head's tip (a head stopped short of what it met, at a charge as much as
    /// 0.4 short), is solid with room above it for its whole body; its body raised a block
    /// and carried on to over that cell's edge meets nothing; and a hop at `speed` carries
    /// its box there before it comes down. A wall two high (which a high head meets in its
    /// upper block), a lintel at head height with open ground under it, a step under a low
    /// ceiling, or a step too far for a hop at a walk, is no step.
    pub fn head_step(&self, world: &impl Surroundings, along: Vec3, speed: f32, jump: f32, step: f32) -> bool {
        let half = self.size.0 * 0.5;
        if self.reach <= half + 0.05 || along.dot(self.forward()) <= 0.7 {
            return false;
        }
        let forward = self.forward();
        let feet = (self.position.y + 0.01).floor() as i32;
        // Nearest first, so on a flight of steps it is the first step, not the next.
        let far = self.reach + 0.5;
        let samples = ((far - half) / 0.1).ceil() as i32;
        // Ground under its front and under every sample short of the step: a block across a
        // gap from it (a trench, or a gap before a fence two high) is no stair up to what is
        // beyond.
        // On a slab or a sill it stands in its feet's cell, on the piece.
        let floored = |cell: IVec3| world.footing(cell - IVec3::Y) || !world.solid(cell) && !world.boxes(cell).is_empty();
        let front = self.position + forward * (half - 0.01);
        if !floored(IVec3::new(front.x.floor() as i32, feet, front.z.floor() as i32)) {
            return false;
        }
        let mut met = None;
        for out in (0..=samples).map(|k| half + (far - half) * k as f32 / samples as f32) {
            let at = self.position + forward * out;
            let cell = IVec3::new(at.x.floor() as i32, feet, at.z.floor() as i32);
            if world.solid(cell) {
                // A wall with no footing on it (a railing's) is no step.
                if !world.footing(cell) {
                    return false;
                }
                met = Some((out, cell));
                break;
            }
            if !floored(cell) {
                return false;
            }
        }
        let Some((out, cell)) = met else { return false };
        // A step it walks up (a slab, a stair's tread, no higher over its feet than `step`)
        // is no step to hop: it walks on and up (`step_up`).
        // A low piece on top of the step (a slab on a block) comes with it.
        let tops = world.boxes(cell).iter().map(|b| cell.y as f32 + b.max.y);
        let over = if world.solid(cell + IVec3::Y) { &[][..] } else { world.boxes(cell + IVec3::Y) };
        if tops.chain(over.iter().map(|b| cell.y as f32 + 1.0 + b.max.y)).all(|top| top - self.position.y <= step) {
            return false;
        }
        // Held off by its head, its box may be as much as its head's reach short of the
        // step: the hop must carry it that far and a little over.
        if speed * 2.0 * jump / GRAVITY < out.max(self.reach) - half + 0.3 {
            return false;
        }
        let mut over = self.aabb();
        over.translate(Vec3::Y * 1.05 + forward * (out - half + 0.3));
        (1..=self.size.1.ceil() as i32).all(|up| !world.solid(cell + IVec3::Y * up)) && !over.overlaps(|p| world.boxes(p))
    }

    /// Where its head (and the neck on the way to it) is, beyond its box: nowhere for a
    /// head within it.
    fn head_points(&self) -> impl Iterator<Item = Vec3> + '_ {
        let beyond = self.reach > self.size.0 * 0.5 + 0.05;
        let forward = self.forward();
        [(1.0, 1.0), (1.0, 0.7), (0.75, 0.85)]
            .into_iter()
            .filter(move |_| beyond)
            .map(move |(out, up)| self.position + forward * self.reach * out + Vec3::Y * ((self.head * up).max(0.2) + self.lift))
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
    /// Whether a cell stands in the way at a body's feet: a whole block, or a shape taller
    /// than a half step (a post, a stair, a top slab). A lower slab is not: it is walked up
    /// onto (`Movement::step`), and the way-finder and the look ahead take it for open
    /// ground.
    fn solid(&self, cell: IVec3) -> bool;
    fn water(&self, cell: IVec3) -> bool;
    /// Whether a cell is lava (or another fluid that burns): a wall to a land animal at any
    /// depth, as water two deep is. None by default.
    fn lava(&self, _cell: IVec3) -> bool {
        false
    }

    /// Whether `cell` is something to stand on, as well as solid: a cell that is only a
    /// wall (over a railing, which stands two high to a creature) or a railing's own narrow
    /// top is no floor, no step to hop onto and no landing.
    fn footing(&self, cell: IVec3) -> bool {
        self.solid(cell)
    }

    /// What a cell stops a body with, in blocks from its lower corner
    /// (`BlockRegistry::collision`): by default the whole cell if it is solid.
    fn boxes(&self, cell: IVec3) -> &[Aabb] {
        if self.solid(cell) { &CUBE } else { &[] }
    }
}

/// How high a shape stands before it is in a body's way at its feet, in blocks: a lower
/// slab is walked up onto (`Movement::step`, never less), and is open ground to the
/// way-finder.
pub const HALF_STEP: f32 = 0.5 + 1e-3;

/// The most a body lifts its head over what it meets (`Body::lift`): half a block, a
/// stair's tread.
pub const HEAD_LIFT: f32 = 0.5;

/// Whether a point is in something: in one of its cell's boxes (a whole block's cell, as
/// the cell it is in).
pub fn inside(world: &impl Surroundings, p: Vec3) -> bool {
    let cell = p.floor().as_ivec3();
    let at = p - cell.as_vec3();
    world.boxes(cell).iter().any(|b| (0..3).all(|k| at[k] >= b.min[k] && at[k] < b.max[k]))
}

/// Where a box at `start` moving by `motion` ends if it first rises by up to `step`, then goes
/// across, then comes down onto what it stepped onto, with which axes across stopped it;
/// none if it would come down on nothing (as a player steps, `movement::advance`).
fn step_up(start: Aabb, motion: Vec3, step: f32, world: &impl Surroundings) -> Option<(Aabb, [bool; 3])> {
    let boxes = |p: IVec3| world.boxes(p);
    let mut aabb = start;
    move_axis_boxes(&mut aabb, 1, step, &boxes);
    let risen = aabb.min.y - start.min.y;
    let across = [move_axis_boxes(&mut aabb, 0, motion.x, &boxes), false, move_axis_boxes(&mut aabb, 2, motion.z, &boxes)];
    let landed = move_axis_boxes(&mut aabb, 1, -(risen - motion.y.min(0.0)), &boxes);
    landed.then_some((aabb, across))
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
    // A step of no time would never come down.
    let dt = dt.max(1e-3);
    let (mut v, mut up) = (jump, 0.0);
    while v > 0.0 {
        up += v * dt;
        v -= GRAVITY * dt;
    }
    up
}

/// The least jump that rises `rise` blocks, stepped as `dt` steps it.
fn rise_speed(rise: f32, dt: f32) -> f32 {
    let (mut low, mut high) = (0.0, 30.0);
    for _ in 0..24 {
        let mid = (low + high) / 2.0;
        if hop_height(mid, dt) >= rise { high = mid } else { low = mid }
    }
    high
}

/// The least jump that hops up a single block, stepped as `dt` steps it: anything that
/// jumps at all gets up one, however low its own jump (its jump is for leaps and hops).
fn climb_speed(dt: f32) -> f32 {
    rise_speed(1.1, dt)
}

/// How fast a hop up a step goes, stepped as `dt` steps it: its own jump, rising no higher
/// than the world's `step_rise` (`creature::Climbing`, carried on its `Movement`), or the
/// least that gets up a block (`climb_speed`). Solved for `dt` as the climb is, so the rise
/// holds at any step. Steering (`open_way`) and the hop (`step`) judge a step by the same
/// hop.
pub fn hop_speed(movement: &Movement, dt: f32) -> f32 {
    movement.jump.min(rise_speed(movement.step_rise, dt)).max(climb_speed(dt))
}

/// How quickly a body on the ground (or in water, or flying) takes up the speed it wants:
/// the share of the difference made up in a second.
pub const GROUND_CONTROL: f32 = 10.0;

/// Seconds a chase from a standstill over `way` blocks at `speed` takes with a hop up a
/// block on the way, by the body's own physics: the way at its speed; the hop's time in the
/// air at its own jump (`hop_speed`), counted as making no way; and its gathering speed from
/// a standstill before and after the hop (`GROUND_CONTROL`, made up a share a step).
pub fn chase_with_hop(movement: &Movement, way: f32, speed: f32, dt: f32) -> f32 {
    let rise = hop_speed(movement, dt);
    let airborne = (rise + (rise * rise - 2.0 * GRAVITY).max(0.0).sqrt()) / GRAVITY;
    let share = (GROUND_CONTROL * dt).min(1.0);
    let gathering = dt * (1.0 - share) / share;
    way / speed.max(0.1) + airborne + 2.0 * gathering
}

/// Whether the body can go a little way along `heading` (horizontal, unit length): sliding
/// along a wall counts, and so does a single step up for anything that jumps; its head must
/// not meet rock there, and on the ground it must not step off a drop deeper than
/// `SAFE_DROP` or, a land animal on dry land, into water two deep. `pace` is the speed it
/// goes at: a step its head meets is up only for a hop that carries it there at that pace;
/// `dt` is how long a `step` is, which the hop is judged by.
pub fn open_way(body: &Body, world: &impl Surroundings, movement: &Movement, water: Water, heading: Vec3, pace: f32, dt: f32) -> bool {
    let probe = body.size.0 * 0.5 + 0.3;
    let boxes = |p: IVec3| world.boxes(p);
    let goes = |lift: f32| {
        let mut aabb = body.aabb();
        aabb.translate(Vec3::Y * lift);
        if lift > 0.0 && aabb.overlaps(boxes) {
            return None;
        }
        let start = aabb.min;
        // Up a half step as it walks (a slab), as `step` takes it.
        let blocked = move_and_slide_boxes(&mut aabb, heading * probe, boxes);
        if lift == 0.0 && (blocked[0] || blocked[2]) && movement.step > 0.0 {
            let mut raised = body.aabb();
            raised.translate(Vec3::Y * movement.step);
            if !raised.overlaps(boxes) {
                let mut over = raised;
                move_and_slide_boxes(&mut over, heading * probe, boxes);
                if (over.min - start).dot(heading) > (aabb.min - start).dot(heading) + 1e-3 {
                    aabb = over;
                }
            }
        }
        let gone = (aabb.min - start).dot(heading);
        (gone >= probe * 0.5).then(|| Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0))
    };
    // Its head clear where it gets to. Its box meeting a step, the way is up it; so it is
    // where it gets most of the way at its level but its head meets a step (`head_step`),
    // not round it. A high head at a wall two high, or under a lintel, finds no way there.
    let ahead = |to: Vec3| Body { position: to, yaw: yaw_of(heading), ..*body };
    // Its head clear there facing the way it goes, or, going on the way it faces, still
    // facing that way: `step` holds its facing where its turned head would meet something
    // (edging along a wall, or into a gap with a fence across the corner before it).
    let onward = heading.dot(body.forward()) >= -1e-4;
    let clear = |to: Vec3| !ahead(to).head_blocked(world) || onward && !Body { position: to, ..*body }.head_blocked(world);
    let level = goes(0.0);
    let up = movement.jump > 0.0 && level.is_none_or(|to| ahead(to).head_step(world, heading, pace, hop_speed(movement, dt), movement.step));
    let Some(to) = level.filter(|&to| clear(to)).or_else(|| if up { goes(1.05).filter(|&to| !ahead(to).head_blocked(world)) } else { None }) else {
        return false;
    };
    // What it would come down on along its way, from here to as far ahead as it carries
    // on before it can turn (a running deer, or a hare mid-hop, goes a block or so),
    // against what it stands on now: the drop between them, and water there, judged the
    // same on the ground or in the air. A wall along the way is ground to no one.
    let reach = SAFE_DROP + 4;
    // A low piece in a cell (a slab, below a body's feet as `Surroundings::solid` goes) is
    // that cell's floor: on a slab bridge, what is below it is no ground to judge by.
    let low = |c: IVec3| !world.solid(c) && !world.boxes(c).is_empty();
    let ground = |from: IVec3| (0..=reach).map(|k| from - IVec3::Y * k).find(|&c| world.water(c) || world.lava(c) || low(c) || world.solid(c - IVec3::Y));
    // Ahead, a railing's top, or the wall over it, is no landing: what comes down there is
    // off the way, as over a drop too deep.
    let landing = |from: IVec3| ground(from).filter(|&c| world.water(c) || world.lava(c) || low(c) || world.footing(c - IVec3::Y));
    // What it stands on is the highest ground under any part of its box (at an edge, its
    // middle may already be over the drop), whatever it is: one already up on a railing,
    // or on the wall over it, still judges the drop beyond from there.
    let half = body.size.0 * 0.5 - 0.05;
    let Some(now) = [(0.0, 0.0), (half, half), (half, -half), (-half, half), (-half, -half)]
        .into_iter()
        .filter_map(|(x, z)| ground((body.position + Vec3::new(x, 0.01, z)).floor().as_ivec3()))
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
    // Lava is a wall to anything that walks, unless it is already in it: then it walks out,
    // and no deeper in.
    let lava_kind = matches!(water, Water::Land | Water::Amphibious);
    let in_lava = lava_kind && (world.lava(feet) || world.lava(feet - IVec3::Y));
    let burns = lava_kind && !in_lava;
    // In lava a block deep, it goes no deeper: lava two deep is shut to it, as deep water is.
    let lava_deep = |c: IVec3| world.lava(c) && world.lava(c - IVec3::Y);
    let shallow = in_lava && !lava_deep(feet) && !lava_deep(feet + IVec3::Y);
    // Along the way it really goes, sliding along a wall as its box would, a step at a
    // time; ahead of its middle by half its width.
    // Down into a space it would hang in (a trench too short for a long head to come down
    // into with its head clear): no way at a walk, or it walks in and stands stuck there.
    // Judged once, at the first place its way goes down, over all of that space along its
    // way it fits in: a pit with room to land short of its far wall is open. Running or
    // chasing, a creature drops in (a hop carries it, and its brain gives a held way up);
    // into water it swims, its head up.
    // Down is below where its feet are: at a lip its box's corners may all be over the drop
    // (`now`) while its edge still stands on it.
    let feet = (body.position.y + 0.01).floor() as i32;
    // `pit`: judged for a pit too (below); never from under its own middle, or one whose
    // middle is over a pit's lip would find no way at all, back onto the bank either.
    // The first landing deeper than its climb is judged for a pit, once: a dip it would
    // step back up from, or a crack too narrow to come down in, leaves the next for it.
    let pit_judged = std::cell::Cell::new(false);
    let held = |middle: Vec3, then: IVec3, trench: bool, pit: bool| {
        if then.y >= feet {
            return false;
        }
        let at = |k: i32| Vec3::new(middle.x, then.y as f32, middle.z) + heading * (0.1 * k as f32);
        // Too narrow along its way for its box (a crack, a ditch it straddles): it never
        // comes down in it.
        let open = (-4..=4).filter(|&k| !world.solid((at(k * 5) + Vec3::Y * 0.01).floor().as_ivec3())).count() as f32 * 0.5;
        if open < body.size.0 {
            return false;
        }
        // Deeper than it could climb back, unless in a chase (`Body::bold`), only onto open
        // ground: never into a pit it would not get out of (a hole a player dug two or three
        // deep), at any pace; a pit with water in its floor too, as a pool it would swim up
        // out of only if its top is in reach.
        let climb = (movement.jump > 0.0) as i32;
        if pit && !body.bold && feet - then.y > climb {
            pit_judged.set(true);
            let walker = Walker {
                height: body.size.1.ceil() as i32,
                climbs: climb > 0,
                step: movement.step,
                wary: water == Water::Land,
                span: Walker::span_of(body.size.0),
                drop: climb,
            };
            if !path::way_out(world, &walker, walker.corner(Vec3::new(middle.x, then.y as f32 + 0.01, middle.z)), feet) {
                return true;
            }
        }
        if !trench || pace > movement.walk || world.water(then) {
            return false;
        }
        let landed = |k: i32| Body { position: at(k), yaw: yaw_of(heading), ..*body };
        let fits = |k: i32| !landed(k).aabb().overlaps(boxes);
        let Some(near) = (0..=20).map(|k| if k % 2 == 1 { -(k + 1) / 2 } else { k / 2 }).find(|&k| fits(k)) else {
            return false;
        };
        // Down there, nowhere it fits with its head clear, it would hang where its head stops
        // it coming down. Open ground below a ledge is a way down, however far it walks on.
        let first = (-20..near).rev().take_while(|&k| fits(k)).last().unwrap_or(near);
        let last = (near..=20).take_while(|&k| fits(k)).last().unwrap_or(near);
        (first..=last).find(|&k| !landed(k).head_blocked(world)).is_none()
    };
    // From under its own middle too: edging over a trench's lip, it may stand with its middle
    // over the trench, and where it gets to may be past it.
    let under = (body.position + Vec3::Y * 0.01).floor().as_ivec3();
    let mut judged = false;
    if !world.solid(under)
        && let Some(then) = landing(under)
        && then.y < feet
    {
        judged = true;
        if held(body.position, then, true, false) {
            return false;
        }
    }
    let samples = (ahead / 0.4).ceil().max(1.0) as i32;
    let step = heading * (ahead / samples as f32);
    let mut aabb = Aabb::standing(to, body.size.0, body.size.1);
    (0..=samples).all(|i| {
        if i > 0 {
            move_and_slide_boxes(&mut aabb, step, boxes);
        }
        let middle = Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0);
        let cell = (middle + Vec3::Y * 0.01).floor().as_ivec3();
        if world.solid(cell) {
            return true;
        }
        match landing(cell) {
            None => false,
            Some(then) => {
                let first_down = then.y < feet && !std::mem::replace(&mut judged, true);
                let first_pit = then.y < feet && !pit_judged.get();
                now - then.y <= SAFE_DROP
                    && !(wary && world.water(then) && world.water(then - IVec3::Y))
                    && !(burns && world.lava(then))
                    && !(in_lava && world.lava(then) && then.y < now)
                    && !(shallow && lava_deep(then))
                    && !((first_down || first_pit) && held(middle, then, first_down, first_pit))
            }
        }
    })
}

/// The way a walking body goes for a `wish` (horizontal, unit length): the wish itself
/// while it is open (`open_way`), else the nearest turn from it that is, to the side it
/// already took (`side`, 1 or -1; 0 for none yet: the side it faces more). Returns the
/// heading and the side taken, 0 when it goes straight; boxed in, it stands (no heading,
/// and 0) rather than press into a wall or step off a cliff. This is
/// how anything walking gets round a wall, a corner or a tree rather than pressing into it.
/// `pace` is the speed it goes at (`open_way`).
pub fn steer(body: &Body, world: &impl Surroundings, movement: &Movement, water: Water, wish: Vec3, side: f32, pace: f32, dt: f32) -> (Vec3, f32) {
    if wish == Vec3::ZERO || open_way(body, world, movement, water, wish, pace, dt) {
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
    tries.into_iter().map(|(sign, degrees)| (turned(sign, degrees), sign)).find(|&(heading, _)| open_way(body, world, movement, water, heading, pace, dt)).unwrap_or((Vec3::ZERO, 0.0))
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
    let control = if body.on_ground || body.in_water || floating || flying { GROUND_CONTROL } else if flier { 6.0 } else { 2.0 };
    let horizontal = body.velocity.with_y(0.0).lerp(target.with_y(0.0), (control * dt).min(1.0));
    body.velocity = horizontal.with_y(body.velocity.y);

    if flying {
        // Keep its height over the land here and just ahead, so it rises over a hill or a
        // tree before it reaches it.
        let ahead = (body.position + facing * LOOK_AHEAD).floor().as_ivec3();
        // A slab roof is land under it too, though no wall to its feet.
        let top = |column: IVec3, from: i32| (0..from + 24).map(|d| column + IVec3::Y * (from - d)).find(|&c| world.solid(c) || world.water(c) || !world.boxes(c).is_empty()).map(|c| c.y as f32 + 1.0);
        let reach = movement.fly.ceil() as i32 + 2;
        let ground = top(feet, 0).into_iter().chain(top(ahead, reach)).fold(f32::MIN, f32::max);
        let want = if ground == f32::MIN { body.position.y } else { ground + movement.fly };
        body.velocity.y = ((want - body.position.y) * 1.5).clamp(-DESCEND, CLIMB);
    } else if flier && !body.on_ground && !body.in_water {
        // Flutters down to land.
        body.velocity.y = (body.velocity.y - GRAVITY * dt).max(-FLUTTER_DOWN);
    } else if floating {
        // Float above the ground, bobbing, rising over what is in the way.
        // A slab roof is ground under it, as to a flier.
        let ground = (0..8).map(|d| feet - IVec3::Y * d).find(|&c| world.solid(c) || !world.boxes(c).is_empty()).map_or(body.position.y - 8.0, |c| c.y as f32 + 1.0);
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
    // its feet, not only its head stopped at a door frame, with ground under its front.
    // Paddling at a bank, anything that walks hauls itself out the same way.
    let paddling = body.in_water && water != Water::Swim && !floating && !flying;
    if (body.on_ground && movement.jump > 0.0 || paddling) && body.against_wall && along != Vec3::ZERO {
        let (mut level, mut raised) = (body.aabb(), body.aabb());
        level.translate(along * 0.3);
        raised.translate(Vec3::Y * 1.05 + along * 0.3);
        // Ground under its front: a body standing on a block's edge with its front out over a
        // gap (a deer on a block beside a fence a block off) takes the fence for no step,
        // or it would leap onto a fence two high. Paddling, it hauls itself out of the water.
        let front = body.position + along * (body.size.0 * 0.5 - 0.01);
        let foot = Vec3::new(front.x, body.position.y - 0.01, front.z);
        // On a slab or a sill, its foot is in the piece's own cell.
        let footed = paddling || inside(world, foot) && (world.footing(foot.floor().as_ivec3()) || !world.solid(foot.floor().as_ivec3()));
        // A half step it walks up (`step_up`) is no step to hop; a hopper, in the air at
        // every hop, walks up nothing, and hops it.
        let mut stepped = level;
        stepped.translate(Vec3::Y * movement.step);
        let walks_up = movement.style != Style::Hopper && !stepped.overlaps(|p| world.boxes(p));
        // What it meets is a step only if all of it is footing: the wall over a railing
        // beside it is no step up to walk on air over the railing.
        let boxed = footed && level.overlaps(|p| world.boxes(p)) && !walks_up && !level.intersects(|p| world.solid(p) && !world.footing(p));
        // A long head (a boar's snout, a badger's) stops the body short of a step its box
        // never meets (`Body::head_step`): it hops all the same. Either hop springs on toward
        // the step at its pace, its way along having been stopped at the step: else it goes
        // nearly straight up and, slow (a tortoise), comes down short of the step's edge,
        // where it was.
        let head_step = !boxed && body.head_step(world, along, target.length(), hop_speed(movement, dt), movement.step);
        // Room for it a block up, and in no cell there in the way at a body's feet
        // (`Surroundings::solid`): a lattice window on a wall's top course, however thin, is
        // the wall's second block, and nothing hops onto the sill beside it.
        let room = !raised.overlaps(|p| world.boxes(p)) && !raised.intersects(|p| world.solid(p));
        if (boxed || head_step) && room {
            body.velocity.y = hop_speed(movement, dt);
            body.velocity.x = target.x;
            body.velocity.z = target.z;
        }
    }
    // Hoppers move in hops whenever they move, and so do fliers on the ground: each hop
    // springs the way it wants to go, not the way the last one carried it (it can hardly
    // turn in the air, so a hop still carrying it at a cliff edge would go over).
    // A step within its hop (a stair's tread, a slab, a block), with ground under its front,
    // room a block up and nothing in the way there, it hops up onto: its everyday hop clears less than a half
    // step, and its head would back it off the riser.
    if matches!(movement.style, Style::Hopper | Style::Flier) && body.on_ground && target.length_squared() > 0.05 && body.velocity.y <= 0.0 {
        let way = target.with_y(0.0).normalize_or_zero();
        let (mut before, mut over) = (body.aabb(), body.aabb());
        before.translate(way * 0.6 + Vec3::Y * 0.01);
        over.translate(way * 0.6 + Vec3::Y * 1.05);
        // Ground under its front, as for any hop up: a block across a gap is no step.
        let front = body.position + way * (body.size.0 * 0.5 - 0.01);
        let footed = inside(world, Vec3::new(front.x, body.position.y - 0.01, front.z));
        // All of what it meets is footing, as for any hop up: the wall over a railing is no step.
        let step = footed && before.overlaps(|p| world.boxes(p)) && !before.intersects(|p| world.solid(p) && !world.footing(p)) && !over.overlaps(|p| world.boxes(p)) && !over.intersects(|p| world.solid(p));
        body.velocity = target.with_y(if step && movement.jump > 0.0 { hop_speed(movement, dt) } else { movement.jump.min(6.0) * 0.75 });
    }

    let start_box = body.aabb();
    let mut aabb = start_box;
    let motion = body.velocity * dt;
    let mut blocked = move_and_slide_boxes(&mut aabb, motion, |p| world.boxes(p));
    // Stopped across on the ground: up onto what stopped it if it is a half step (a slab,
    // a stair's tread) and the body fits there, as a player steps; whole blocks it hops.
    if (blocked[0] || blocked[2])
        && body.on_ground
        && motion.y <= 0.0
        && movement.step > 0.0
        && !floating
        && !flying
        && let Some((stepped, across)) = step_up(start_box, motion, movement.step, world)
    {
        let gone = |b: &Aabb| (b.min - start_box.min).with_y(0.0).length_squared();
        let freed = [0, 2].into_iter().any(|k| blocked[k] && !across[k]);
        // Only where it rose onto something: risen, it may be stopped another way (a long
        // body under a lintel) and slide where the walk at its level would not have taken it.
        if stepped.min.y > start_box.min.y + 1e-4 && (freed || gone(&stepped) > gone(&aabb) + 1e-8) {
            aabb = stepped;
            blocked = [across[0], true, across[2]];
        }
    }
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
    // Its head as low as it goes clear: lifted over a tread or a course before it, or let
    // down again past it. Nowhere clear within its lift, it holds it as it was.
    if body.reach > body.size.0 * 0.5 + 0.05
        && let Some(lift) = [0.0, HEAD_LIFT * 0.5, HEAD_LIFT].into_iter().find(|&lift| !Body { lift, ..*body }.head_blocked(world))
    {
        body.lift = lift;
    }
    if !head_was_blocked && body.head_blocked(world) {
        let (moved, turned) = (body.position, body.yaw);
        let fits = |at: Vec3| !Aabb::standing(at, body.size.0, body.size.1).overlaps(|p| world.boxes(p));
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
            // Rising from a hop, only its move along is undone, and it keeps its pace: once
            // its head is up over the step's edge it goes on. Coming down with its head into
            // what is before it (the next riser of a stair), it backs to where its head is
            // clear at the height it came down to, or, with nowhere clear there, is put back
            // whole where its head was clear.
            let rising = body.velocity.y > 0.0;
            // Coming down, it backs as far as its head reaches to land clear of the riser.
            let backs = if rising || body.on_ground { 4 } else { (body.reach / 0.12).ceil() as i32 + 1 };
            if clear(back, turned) {
                body.position = back;
            } else if let Some(at) = (1..=backs).map(|k| moved - body.forward() * (0.12 * k as f32)).find(|&at| clear(at, turned)) {
                body.position = at;
            } else if let Some(yaw) = (body.velocity.y < -0.5)
                .then(|| [0.0f32, 30.0, -30.0, 60.0, -60.0, 90.0, -90.0].map(|d| start_yaw + d.to_radians()).into_iter().find(|&yaw| clear(back, yaw)))
                .flatten()
            {
                // Falling, it twists to where its head is clear and goes on down (a long
                // head in a trench a block wide turns along it) rather than be held in the
                // air where it was.
                body.position = back;
                body.yaw = yaw;
            } else if !rising && clear(start, turned) {
                // Put back on what it stood or hung on: on the ground, or it falls into
                // the same place again next step and hangs there.
                body.position = start;
                body.velocity.y = 0.0;
                body.on_ground = true;
            } else if turned != start_yaw && fits(back) {
                // Committed to its way round, it keeps turning.
                body.position = back;
            } else {
                body.yaw = start_yaw;
                if fits(back) {
                    body.position = back;
                }
            }
            if !rising {
                body.velocity.x = 0.0;
                body.velocity.z = 0.0;
            }
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
        let (heading, side) = steer(&body, &wall, &movement, Water::Land, Vec3::X, 0.0, 2.0, 0.05);
        assert!(side != 0.0 && heading.dot(Vec3::X).abs() < 0.5, "it pressed on into the wall: {heading}");
        assert!(heading.z > 0.0, "it went round the way it faced less: {heading}");
        // Going round, it keeps to its side though the other is as open.
        let (again, kept) = steer(&body, &wall, &movement, Water::Land, Vec3::X, -side, 2.0, 0.05);
        assert_eq!(kept, -side, "it changed sides going round");
        assert!(again.z < 0.0);
        // An open way is taken as it is.
        assert_eq!(steer(&body, &wall, &movement, Water::Land, -Vec3::X, side, 2.0, 0.05), (-Vec3::X, 0.0));
        // Walking it, it slides along and is never pressed still.
        for _ in 0..40 {
            let (heading, _) = steer(&body, &wall, &movement, Water::Land, Vec3::X, side, 2.0, 0.05);
            step(&mut body, &wall, &movement, Water::Land, Drive { wish: heading, speed: 2.0, face: None }, 0.05);
        }
        assert!(body.position.z > 2.5, "it barely moved along the wall: {}", body.position);
        assert!(body.position.x + 0.3 <= 3.0 + 1e-3, "it went into the wall");
    }

    /// A great leaper's hop up a step rises the world's `step_rise` and no higher, at any
    /// step length; a low jumper's still gets up a block.
    #[test]
    fn a_hop_up_a_step_rises_no_higher_than_the_step_rise_at_any_dt() {
        for step_rise in [1.2, 1.45, 1.9] {
            for dt in [0.005, 1.0 / 60.0, 0.05, 0.1] {
                let leaper = Movement { jump: 11.0, step_rise, ..Movement::default() };
                let rise = hop_height(hop_speed(&leaper, dt), dt);
                assert!((step_rise..step_rise + 0.05).contains(&rise), "rise {rise} for {step_rise} at {dt}");
                let low = Movement { jump: 3.0, step_rise, ..Movement::default() };
                assert!(hop_height(hop_speed(&low, dt), dt) >= 1.1);
            }
        }
    }

    #[test]
    fn anything_that_jumps_gets_up_a_single_block_and_no_higher() {
        let step_up = Ledge { high: 1 };
        let body = Body::new(Vec3::new(2.7, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        let (low, none) = (Movement { jump: 6.0, ..Movement::default() }, Movement { jump: 0.0, ..Movement::default() });
        assert!(hop_height(6.0, 0.05) < 1.0 && hop_height(climb_speed(0.05), 0.05) >= 1.1);
        assert!(open_way(&body, &step_up, &low, Water::Land, Vec3::X, 3.0, 0.05), "a low jumper found a single block a wall");
        assert!(!open_way(&body, &step_up, &none, Water::Land, Vec3::X, 3.0, 0.05), "a block was open to one that cannot jump");
        assert!(!open_way(&body, &Ledge { high: 2 }, &low, Water::Land, Vec3::X, 3.0, 0.05), "a wall two high was open");
        // And it really gets up, though its own jump would not lift it a block.
        let mut walker = Body::new(Vec3::new(1.5, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        walker.on_ground = true;
        for _ in 0..60 {
            step(&mut walker, &step_up, &low, Water::Land, Drive { wish: Vec3::X, speed: 2.0, face: None }, 0.05);
        }
        assert!(walker.position.y >= 1.0 && walker.position.x > 3.5, "it never got up the step: {}", walker.position);
    }

    /// A badger-shaped body (a low box, its head well ahead of it) walking straight at a
    /// step meets it with its head while its box is still short of it: the way is up, not
    /// round, and it gets up without turning. Before: its head stopped it, the step read as
    /// a wall, and it went round or hopped on the spot (a wolf took 4 s to climb three).
    #[test]
    fn a_long_head_meeting_a_step_first_goes_up_it_not_round() {
        let step_up = Ledge { high: 1 };
        let movement = Movement { jump: 6.5, ..Movement::default() };
        let mut body = Body::new(Vec3::new(1.0, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.45));
        (body.reach, body.head, body.on_ground) = (0.75, 0.3, true);
        let short = Body { position: Vec3::new(2.3, 0.0, 0.5), ..body };
        assert!(short.head_blocked(&step_up) && !Aabb::standing(short.position, 0.6, 0.45).intersects(|p| step_up.solid(p)), "the fixture means its head to meet the step first");
        assert!(open_way(&short, &step_up, &movement, Water::Land, Vec3::X, 3.0, 0.05), "the step read as a wall to its head");
        for i in 0..30 {
            step(&mut body, &step_up, &movement, Water::Land, Drive { wish: Vec3::X, speed: 3.0, face: None }, 0.05);
            assert!(!body.head_blocked(&step_up), "its head went into the step {i} steps on, at {}", body.position);
        }
        assert!(body.position.y >= 1.0 && body.position.x > 3.3, "it never got up the step in 1.5 s: {}", body.position);
        assert!((body.yaw - yaw_of(Vec3::X)).abs() < 0.01, "it turned at the step: yaw {}", body.yaw);
    }

    /// A body with its head carried high (an aurochs, a deer) walking at a wall two blocks
    /// high: its head meets the wall's upper block, which a step up does not clear, so the
    /// wall is a wall. Before: its head lifted a block was over the wall, so its head was
    /// let into the wall as if at a step, and a way at the wall read as open.
    #[test]
    fn a_high_head_at_a_wall_two_high_takes_it_for_a_wall_not_a_step() {
        let wall = Ledge { high: 2 };
        let movement = Movement { jump: 6.5, ..Movement::default() };
        let mut body = Body::new(Vec3::new(1.0, 0.0, 0.5), yaw_of(Vec3::X), (1.0, 1.4));
        (body.reach, body.head, body.on_ground) = (1.1, 1.5, true);
        let short = Body { position: Vec3::new(2.0, 0.0, 0.5), ..body };
        assert!(short.head_blocked(&wall) && !Aabb::standing(short.position, 1.0, 1.4).intersects(|p| wall.solid(p)), "the fixture means its head to meet the wall first");
        assert!(!Body { position: short.position + Vec3::Y * 1.05, ..short }.head_blocked(&wall), "the fixture means its head a block up to be over the wall");
        assert!(!open_way(&short, &wall, &movement, Water::Land, Vec3::X, 3.0, 0.05), "the wall read as open");
        for i in 0..40 {
            step(&mut body, &wall, &movement, Water::Land, Drive { wish: Vec3::X, speed: 3.0, face: None }, 0.05);
            assert!(!body.head_blocked(&wall), "its head went into the wall {i} steps on, at {}", body.position);
            assert!(body.position.y < 0.5, "it hopped at the wall, {i} steps on, at {}", body.position);
        }
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
        for i in 0..80 {
            step(&mut boar, &step_up, &movement, Water::Land, Drive { wish: Vec3::X, speed: 4.0, face: None }, 0.05);
            assert!(!boar.head_blocked(&step_up), "its snout went into the step {i} steps on, at {}", boar.position);
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
        for i in 0..80 {
            step(&mut badger, &step_up, &movement, Water::Land, Drive { wish: Vec3::X, speed: 3.0, face: None }, 0.05);
            assert!(!badger.head_blocked(&step_up), "its snout went into the step {i} steps on, at {}", badger.position);
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

    /// A step or wall at `x >= 3` (`high` blocks), and a roof or beam: solid at `y == roof`
    /// for `x` in `roof_x`.
    struct Roofed {
        high: i32,
        roof: i32,
        roof_x: (i32, i32),
    }

    impl Surroundings for Roofed {
        fn solid(&self, cell: IVec3) -> bool {
            cell.y < 0 || (cell.x >= 3 && cell.y < self.high) || (cell.y == self.roof && (self.roof_x.0..self.roof_x.1).contains(&cell.x))
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    /// `ticks` of `body` driven along +x (or standing facing +x): whether its head was ever
    /// in a block, its highest, and its hops.
    fn drive(body: &mut Body, world: &impl Surroundings, movement: &Movement, standing: bool, ticks: u32) -> (u32, f32, u32) {
        let (mut inside, mut high, mut hops) = (0, f32::MIN, 0);
        let drive = if standing { Drive { wish: Vec3::ZERO, speed: 0.0, face: Some(Vec3::X) } } else { Drive { wish: Vec3::X, speed: 4.0, face: None } };
        for _ in 0..ticks {
            let was = body.velocity.y;
            step(body, world, movement, Water::Land, drive, 0.05);
            inside += body.head_blocked(world) as u32;
            high = high.max(body.position.y);
            hops += (body.velocity.y > 0.0 && was <= 0.0) as u32;
        }
        (inside, high, hops)
    }

    /// A deer's shape (its head carried high and well ahead) at a wall two high and under a
    /// beam at head height, up against it or standing facing it: neither is a way, it never
    /// hops, and its head never goes in. Before: its head a block up was over both, so the
    /// head stop let it in (196 ticks of 200 driven at the wall) and it hopped.
    #[test]
    fn a_deer_at_a_wall_two_high_or_a_beam_keeps_its_head_out_and_its_feet_down() {
        let wall = Roofed { high: 2, roof: -1, roof_x: (0, 0) };
        let beam = Roofed { high: 0, roof: 1, roof_x: (3, i32::MAX) };
        for jump in [9.5, 12.0] {
            let movement = Movement { jump, ..Movement::default() };
            let deer = || {
                let mut deer = Body::new(Vec3::new(1.82, 0.0, 0.5), yaw_of(Vec3::X), (0.8, 1.5));
                (deer.reach, deer.head, deer.on_ground, deer.against_wall) = (1.17, 1.53, true, true);
                deer
            };
            let check = |name: &str, world: &dyn Fn(&mut Body, bool) -> (u32, f32, u32), open: bool| {
                assert!(!open, "{name} (jump {jump}) read as a way");
                for standing in [false, true] {
                    let mut body = deer();
                    let (inside, high, hops) = world(&mut body, standing);
                    assert!(inside == 0 && high < 1.0, "{name} (jump {jump}, standing {standing}): its head in {inside} ticks of 200, its feet up to {high:.2}, {hops} hops");
                }
            };
            check("a wall two high", &|b: &mut Body, standing| drive(b, &wall, &movement, standing, 200), open_way(&deer(), &wall, &movement, Water::Land, Vec3::X, 3.0, 0.05));
            check("a beam", &|b: &mut Body, standing| drive(b, &beam, &movement, standing, 200), open_way(&deer(), &beam, &movement, Water::Land, Vec3::X, 3.0, 0.05));
        }
    }

    /// A flight of steps a block deep from `x = 3`, `top` high.
    struct Stairs {
        top: i32,
    }

    impl Surroundings for Stairs {
        fn solid(&self, cell: IVec3) -> bool {
            cell.y < 0 || (cell.x >= 3 && cell.y < (cell.x - 2).min(self.top))
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    /// At a walk, a hop cannot carry a long-headed body the way its box sits behind a step
    /// its snout meets: it does not hop at it (and steering goes round), its snout kept out.
    /// Before: Old Tusk walking at a step (his attacks follow at a walk) hopped 15 times in
    /// 10 s and stayed where he was.
    #[test]
    fn a_long_head_walking_at_a_step_does_not_bounce_at_it() {
        let step_up = Ledge { high: 1 };
        let movement = Movement { jump: 7.5, walk: 1.06, ..Movement::default() };
        let mut tusk = Body::new(Vec3::new(0.5, 0.0, 0.5), yaw_of(Vec3::X), (1.14, 1.14));
        (tusk.on_ground, tusk.reach, tusk.head) = (true, 1.41, 0.79);
        let mut hops = 0;
        for i in 0..200 {
            let was = tusk.velocity.y;
            step(&mut tusk, &step_up, &movement, Water::Land, Drive { wish: Vec3::X, speed: 1.06, face: None }, 0.05);
            hops += (tusk.velocity.y > 0.0 && was <= 0.0) as u32;
            assert!(!tusk.head_blocked(&step_up), "its snout went into the step {i} steps on, at {}", tusk.position);
        }
        assert!(hops <= 1, "{hops} hops in 10 s at a walk, at {}", tusk.position);
        let at = Body { position: Vec3::new(3.0 - 0.57 - 0.6, 0.0, 0.5), ..tusk };
        assert!(!open_way(&at, &step_up, &movement, Water::Land, Vec3::X, 1.06, 0.05), "a step too far to hop at a walk read as a way");
    }

    /// Up a flight of steps a block deep, a long head meets the next riser as it lands on
    /// each step: it climbs the flight with its head never in a block, a hound's shape at a
    /// run and a camel's (its head high, meeting the second riser while the first is under
    /// its neck). Before: the head hop let the snout into each riser for 5-6 ticks, and the
    /// camel, judged at the riser past its reach, stalled at the foot.
    #[test]
    fn long_heads_climb_a_flight_with_their_heads_kept_out() {
        let stairs = Stairs { top: 3 };
        for (name, size, reach, head, jump, speed) in [("hound", (0.6, 0.8), 0.95, 0.7, 9.5, 6.0), ("camel", (1.01, 2.02), 1.75, 1.85, 7.0, 6.0)] {
            let movement = Movement { jump, ..Movement::default() };
            let mut body = Body::new(Vec3::new(0.5, 0.0, 0.5), yaw_of(Vec3::X), size);
            (body.on_ground, body.reach, body.head) = (true, reach, head);
            for i in 0..200 {
                step(&mut body, &stairs, &movement, Water::Land, Drive { wish: Vec3::X, speed, face: None }, 0.05);
                assert!(!body.head_blocked(&stairs), "the {name}'s head went into a step {i} steps on, at {}", body.position);
            }
            assert!(body.position.y >= 3.0, "the {name} never got up the flight: at {}", body.position);
        }
    }

    /// A slab bridge over air from x = 0 to 2, and at x = 3 a block with a slab on it: a
    /// step a block up from the bridge.
    struct SlabBridge;

    const SLAB: [Aabb; 1] = [Aabb { min: Vec3::ZERO, max: Vec3::new(1.0, 0.5, 1.0) }];

    impl Surroundings for SlabBridge {
        fn solid(&self, c: IVec3) -> bool {
            c.y < -8 || c == IVec3::new(3, 0, 0)
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
        fn boxes(&self, c: IVec3) -> &[Aabb] {
            if (0..=2).contains(&c.x) && c.y == 0 && c.z == 0 || c == IVec3::new(3, 1, 0) {
                &SLAB
            } else if self.solid(c) {
                &CUBE
            } else {
                &[]
            }
        }
    }

    /// A long head on a slab bridge takes a block up from it for a step to hop, its footing
    /// the slabs it stands on. Before: footing judged as the cell under its feet (the air
    /// under the bridge), it never hopped, and backed off.
    #[test]
    fn a_long_head_on_a_slab_bridge_hops_a_block_up() {
        let movement = Movement { jump: 9.5, ..Movement::default() };
        let mut hound = Body::new(Vec3::new(2.0, 0.5, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        (hound.on_ground, hound.reach, hound.head) = (true, 0.95, 0.7);
        assert!(hound.head_step(&SlabBridge, Vec3::X, 6.0, hop_speed(&movement, 0.05), movement.step), "no hop up from the slab bridge");
    }

    /// A floating body over a slab roof holds its height over the roof. Before: its look
    /// for ground went down through the slab (no wall at the feet) to the floor under it,
    /// and it sank onto the roof.
    #[test]
    fn a_floater_holds_its_height_over_a_slab_roof() {
        struct Roof;
        impl Surroundings for Roof {
            fn solid(&self, c: IVec3) -> bool {
                c.y < 0
            }
            fn water(&self, _: IVec3) -> bool {
                false
            }
            fn boxes(&self, c: IVec3) -> &[Aabb] {
                if c.y == 3 { &SLAB } else if self.solid(c) { &CUBE } else { &[] }
            }
        }
        let movement = Movement { style: Style::Drifter, ..Movement::default() };
        let mut body = Body::new(Vec3::new(0.5, 5.6, 0.5), 0.0, (0.6, 0.8));
        let lowest = (0..100)
            .map(|_| {
                step(&mut body, &Roof, &movement, Water::Land, Drive { wish: Vec3::ZERO, speed: 0.0, face: None }, 0.05);
                body.position.y
            })
            .fold(f32::MAX, f32::min);
        assert!(lowest > 5.0, "it sank toward the slab roof: down to {lowest}");
    }

    /// A wolf's shape under a roof a block up, with a step ahead whose top the roof leaves
    /// no room over: no way up, so its head stops at the step and its feet stay down.
    /// Before: its head clear a block up beyond the roof's end let it into the step (186-195
    /// ticks of 200).
    #[test]
    fn a_wolf_under_a_low_roof_keeps_its_head_out_of_a_step() {
        let world = Roofed { high: 1, roof: 1, roof_x: (i32::MIN, 3) };
        let movement = Movement { jump: 9.5, ..Movement::default() };
        let mut wolf = Body::new(Vec3::new(1.0, 0.0, 0.5), yaw_of(Vec3::X), (0.7, 0.9));
        (wolf.reach, wolf.head, wolf.on_ground) = (1.02, 0.89, true);
        let (inside, high, hops) = drive(&mut wolf, &world, &movement, false, 200);
        assert!(inside == 0 && high < 0.01, "its head in {inside} ticks of 200, its feet up to {high:.2}, {hops} hops, at {}", wolf.position);
    }

    /// A lynx's shape at a step with a block of room over it under a roof: one hop gets it
    /// up. Before: its head's lifted try read a way the hop could not take, and it hopped 17
    /// times in 10 s.
    #[test]
    fn a_lynx_at_a_step_under_a_roof_hops_once() {
        let world = Roofed { high: 1, roof: 2, roof_x: (i32::MIN, i32::MAX) };
        let movement = Movement { jump: 9.5, ..Movement::default() };
        let mut lynx = Body::new(Vec3::new(1.0, 0.0, 0.5), yaw_of(Vec3::X), (0.7, 0.75));
        (lynx.reach, lynx.head, lynx.on_ground) = (0.75, 0.75, true);
        let (_, _, hops) = drive(&mut lynx, &world, &movement, false, 200);
        assert!(hops <= 1, "{hops} hops in 10 s, at {}", lynx.position);
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
        let open = |edge: &Edge, water: Water| open_way(&body, edge, &movement, water, Vec3::X, 3.0, 0.05);
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
        assert!(!open_way(&wading, &Shelf, &movement, Water::Land, Vec3::X, 3.0, 0.05), "wading, it walked on into deep water");
        let mut swimming = Body::new(Vec3::new(3.5, -2.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        swimming.in_water = true;
        assert!(open_way(&swimming, &Shelf, &movement, Water::Land, Vec3::X, 3.0, 0.05), "out of its depth, deeper water was a wall");
        // A step down a block, then a pit three deep and walled all round, both within its look ahead:
        // the step it would climb back up leaves the pit still judged, and shut.
        struct Dip;
        impl Surroundings for Dip {
            fn solid(&self, cell: IVec3) -> bool {
                match (cell.x, cell.z) {
                    (1, _) => cell.y < -1,
                    (2..=3, -1..=1) => cell.y < -3,
                    _ => cell.y < 0,
                }
            }
            fn water(&self, _: IVec3) -> bool {
                false
            }
        }
        let mut walker = Body::new(Vec3::new(1.2, 0.0, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
        walker.on_ground = true;
        assert!(!open_way(&walker, &Dip, &movement, Water::Land, Vec3::X, 3.0, 0.05), "a step down a block left the pit past it open");
        // A slab is its cell's floor: on a slab floor or bridge from planks over a room eight
        // deep, dry or flooded, it walks onto it, along it and back, and its far end is a
        // drop like any other, not ground level with what is far below.
        struct SlabBridge {
            wet: bool,
        }
        impl Surroundings for SlabBridge {
            fn solid(&self, cell: IVec3) -> bool {
                cell.y < -8 || cell.x < 0 && cell.y < 0
            }
            fn water(&self, cell: IVec3) -> bool {
                self.wet && (-8..0).contains(&cell.y) && !self.solid(cell)
            }
            fn boxes(&self, cell: IVec3) -> &[Aabb] {
                const SLAB: [Aabb; 1] = [Aabb { min: Vec3::ZERO, max: Vec3::new(1.0, 0.5, 1.0) }];
                if self.solid(cell) {
                    &CUBE
                } else if cell.y == 0 && (0..4).contains(&cell.x) {
                    &SLAB
                } else {
                    &[]
                }
            }
        }
        for wet in [false, true] {
            let bridge = SlabBridge { wet };
            let standing = |x: f32, y: f32| {
                let mut body = Body::new(Vec3::new(x, y, 0.5), yaw_of(Vec3::X), (0.6, 0.8));
                body.on_ground = true;
                body
            };
            let open = |body: &Body, heading: Vec3| open_way(body, &bridge, &movement, Water::Land, heading, 3.0, 0.05);
            assert!(open(&standing(-0.5, 0.0), Vec3::X), "wet {wet}: from the planks the slab floor was a drop");
            assert!(open(&standing(1.5, 0.5), Vec3::X) && open(&standing(1.5, 0.5), -Vec3::X), "wet {wet}: on the slab floor it found no way along it");
            assert!(!open(&standing(3.6, 0.5), Vec3::X), "wet {wet}: it walked off the slab floor's end");
        }
        // Steered at the edge, it goes along it; boxed in by cliffs, it stands.
        let cliff = Edge { drop: 8, wet: 0 };
        let (heading, side) = steer(&body, &cliff, &movement, Water::Land, Vec3::X, 0.0, 2.0, 0.05);
        assert!(side != 0.0 && heading.x < 0.5, "it went over the edge: {heading}");
        for _ in 0..60 {
            let (heading, _) = steer(&body, &cliff, &movement, Water::Land, Vec3::X, side, 2.0, 0.05);
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
