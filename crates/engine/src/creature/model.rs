//! Creature bodies: boxes, built from a body plan or listed one by one.
//!
//! A body plan ([`Model`]) is a few numbers in `creatures.ron` that this module turns into
//! boxes ([`Part`]): a quadruped's torso, legs, neck, head, muzzle, ears, horns and tail, a
//! bird's tilted body with folded wings, comb and fanned tail, a crawler's jointed legs, a
//! snail's shell and eye stalks. Each box knows what it is ([`Role`]), so the skin painter
//! ([`super::skin`]) can give a head its eyes, a leg its hoof and an ear its pink inside,
//! and how it moves ([`Swing`]), so the client can animate it. A mod that wants a body no
//! plan makes lists its boxes itself ([`Model::Boxes`]).
//!
//! Boxes may be turned at rest (an ear tilted out, a horn sweeping back, a tail hanging):
//! the turn is about the box's own centre, and the swing about its pivot carries the turned
//! box with it. Model space has the feet at the origin and the creature facing −Z.

use glam::{EulerRot, Quat, Vec2, Vec3};
use serde::Deserialize;

/// A body plan from the part library. Sizes are (width, height, length) in blocks.
#[derive(Clone, Debug, Deserialize)]
pub enum Model {
    /// Four legs: rabbits to bears.
    Quadruped {
        body: (f32, f32, f32),
        head: (f32, f32, f32),
        /// Leg length.
        legs: f32,
        /// Ear width and height.
        #[serde(default)]
        ears: Option<(f32, f32)>,
        #[serde(default)]
        tail: Option<(f32, f32, f32)>,
        /// Horn thickness and length; their shape is `horn_shape`.
        #[serde(default)]
        horns: Option<(f32, f32)>,
        #[serde(default)]
        horn_shape: HornShape,
        #[serde(default)]
        snout: Option<(f32, f32, f32)>,
        /// A neck: its length and how far it lifts from level, in degrees. Without one
        /// the head sits on the front of the body.
        #[serde(default)]
        neck: Option<(f32, f32)>,
        /// Big hind legs, as hares and cats have.
        #[serde(default)]
        haunches: bool,
        /// A rise over the shoulders, as a fraction of the body's height: bears, bison.
        #[serde(default)]
        hump: Option<f32>,
        /// Degrees the tail hangs below level (negative: carried up).
        #[serde(default)]
        tail_droop: f32,
        /// Degrees the ears lean out from upright.
        #[serde(default = "ear_lean")]
        ear_lean: f32,
    },
    /// Two legs: birds and walking things.
    Biped {
        body: (f32, f32, f32),
        head: (f32, f32, f32),
        legs: f32,
        /// Arm length; none for birds.
        #[serde(default)]
        arms: Option<f32>,
        #[serde(default)]
        wings: Option<(f32, f32)>,
        #[serde(default)]
        beak: Option<(f32, f32, f32)>,
        #[serde(default)]
        tail: Option<(f32, f32, f32)>,
        /// Degrees the body tilts up at the front: birds stand tilted.
        #[serde(default)]
        tilt: f32,
        /// Degrees the tail lifts above level.
        #[serde(default)]
        tail_lift: f32,
        /// A comb on the head (width, height, length), in the fourth colour.
        #[serde(default)]
        crest: Option<(f32, f32, f32)>,
        /// A wattle under the beak (width, height), in the fourth colour.
        #[serde(default)]
        wattle: Option<(f32, f32)>,
    },
    /// Many legs low to the ground: spiders and crabs. Each leg bends at a knee.
    Crawler { body: (f32, f32, f32), head: (f32, f32, f32), legs: f32, pairs: u8 },
    /// A body with no legs: snails, slugs, wisps.
    Blob {
        body: (f32, f32, f32),
        #[serde(default)]
        head: Option<(f32, f32, f32)>,
        #[serde(default)]
        shell: Option<(f32, f32, f32)>,
        /// Eye stalks on the head, as snails have: their length.
        #[serde(default)]
        stalks: Option<f32>,
        /// Little lights that circle it.
        #[serde(default)]
        motes: u8,
    },
    /// Boxes listed one by one, for bodies no plan makes.
    Boxes(Vec<BoxDef>),
}

fn ear_lean() -> f32 {
    18.0
}

/// How horns grow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum HornShape {
    /// Straight up, leaning out a little: goats, young bulls.
    Straight,
    /// Out to the side, then up and forward: cattle.
    #[default]
    Curved,
    /// Back, down and round: wild sheep.
    Curl,
    /// A beam back and up with tines forward: deer.
    Antlers,
    /// Short and forward from the jaw: boars' tusks.
    Tusks,
}

/// A box of a [`Model::Boxes`] body, as `creatures.ron` writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxDef {
    #[serde(default)]
    pub role: Role,
    pub size: (f32, f32, f32),
    /// Its centre.
    pub at: (f32, f32, f32),
    /// The point it swings about; its centre if not given.
    #[serde(default)]
    pub pivot: Option<(f32, f32, f32)>,
    /// Its turn at rest, in degrees: pitch (about X), yaw (about Y), roll (about Z).
    #[serde(default)]
    pub turn: (f32, f32, f32),
    /// Index into the body's colours.
    #[serde(default)]
    pub colour: u8,
    #[serde(default)]
    pub swing: Swing,
}

/// What a box is, so the skin painter and the animation know what to do with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Role {
    #[default]
    Torso,
    Hump,
    Neck,
    Head,
    Snout,
    Beak,
    Ear,
    Horn,
    Leg,
    Thigh,
    Foot,
    Arm,
    Wing,
    Tail,
    Shell,
    Crest,
    Stalk,
    /// A box that is an eye (a stalk's tip): drawn dark, or bright on a glowing body.
    Eye,
    Mote,
}

/// What a part does when the creature moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub enum Swing {
    #[default]
    Still,
    /// Swings fore and aft with the stride; the phase is 0 or a half step. Hind legs
    /// matter to gaits that move them together (hops, bounds).
    Leg { phase: f32, hind: bool },
    /// Swings about the vertical at the hip, as a crawler's legs do.
    Scuttle { phase: f32 },
    /// Turns toward what the creature attends to and dips when it grazes.
    Head,
    /// Sways behind.
    Tail,
    /// Flaps, on the given side (−1 left, 1 right).
    Wing { side: f32 },
    /// Twitches now and then, on the given side.
    Ear { side: f32 },
    /// Circles the body's centre; the phase is where on the circle.
    Orbit { phase: f32 },
}

/// One box of a body, in model space: feet at the origin, facing −Z (as yaw 0 looks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub size: Vec3,
    /// The box's centre at rest.
    pub centre: Vec3,
    /// Its turn at rest, about its centre.
    pub rest: Quat,
    /// The point it swings about (a leg's hip, the neck).
    pub pivot: Vec3,
    /// Index into the body's colours.
    pub colour: u8,
    pub role: Role,
    pub swing: Swing,
}

impl Part {
    /// A point given in the box's own frame (its centre at the origin, unturned), in model
    /// space at rest.
    pub fn at_rest(&self, local: Vec3) -> Vec3 {
        self.centre + self.rest * local
    }
}

/// The colour index plans use for features: horns, beaks, hooves.
const FEATURE: u8 = 2;
/// The colour index of combs and wattles.
const CREST: u8 = 3;

/// Collects parts, with helpers for the shapes plans share.
struct Builder {
    parts: Vec<Part>,
}

impl Builder {
    fn put(&mut self, role: Role, size: Vec3, centre: Vec3, pivot: Vec3, colour: u8, swing: Swing) {
        self.parts.push(Part { size, centre, rest: Quat::IDENTITY, pivot, colour, role, swing });
    }

    /// A box grown from `base` along `direction`, `length` long and `thickness` across:
    /// its long side is its local Y.
    #[allow(clippy::too_many_arguments)]
    fn grow(&mut self, role: Role, base: Vec3, direction: Vec3, length: f32, thickness: Vec2, pivot: Vec3, colour: u8, swing: Swing) -> Vec3 {
        let direction = direction.normalize_or(Vec3::Y);
        let rest = Quat::from_rotation_arc(Vec3::Y, direction);
        let size = Vec3::new(thickness.x, length, thickness.y);
        self.parts.push(Part { size, centre: base + direction * length / 2.0, rest, pivot, colour, role, swing });
        base + direction * length
    }

    /// A box from `from` to `to`, `thickness` across.
    #[allow(clippy::too_many_arguments)]
    fn span(&mut self, role: Role, from: Vec3, to: Vec3, thickness: f32, pivot: Vec3, colour: u8, swing: Swing) {
        self.grow(role, from, to - from, from.distance(to), Vec2::splat(thickness), pivot, colour, swing);
    }

    /// A chain of segments from `base`, each a direction, a length and a thickness: horns,
    /// antlers, jointed legs. Returns where it ends.
    fn chain(&mut self, role: Role, base: Vec3, segments: &[(Vec3, f32, f32)], pivot: Vec3, colour: u8, swing: Swing) -> Vec3 {
        let mut at = base;
        for &(direction, length, thickness) in segments {
            at = self.grow(role, at, direction, length, Vec2::splat(thickness), pivot, colour, swing);
        }
        at
    }
}


impl Model {
    /// The boxes the plan is made of, scaled by `scale`.
    pub fn parts(&self, scale: f32) -> Vec<Part> {
        let v = |(x, y, z): (f32, f32, f32)| Vec3::new(x, y, z) * scale;
        let mut b = Builder { parts: Vec::new() };
        match self {
            Model::Quadruped { body, head, legs, ears, tail, horns, horn_shape, snout, neck, haunches, hump, tail_droop, ear_lean } => {
                let (body, head, legs) = (v(*body), v(*head), *legs * scale);
                let torso = Vec3::new(0.0, legs + body.y / 2.0, 0.0);
                b.put(Role::Torso, body, torso, torso, 0, Swing::Still);
                if let Some(hump) = *hump {
                    let size = Vec3::new(body.x * 0.86, body.y * hump, body.z * 0.42);
                    let at = Vec3::new(0.0, legs + body.y + size.y / 2.0 - body.y * 0.04, -body.z * 0.22);
                    b.put(Role::Hump, size, at, at, 0, Swing::Still);
                }
                let leg = Vec3::new((body.x * 0.28).max(0.06), legs, (body.z * 0.16).max(0.06));
                for (i, (sx, sz)) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].into_iter().enumerate() {
                    let hind = sz > 0.0;
                    let mut size = leg;
                    if hind && *haunches {
                        size.x *= 1.25;
                        size.z *= 1.6;
                    }
                    let hip = Vec3::new(sx * (body.x / 2.0 - size.x / 2.0), legs, sz * (body.z / 2.0 - size.z / 2.0));
                    let phase = if i == 0 || i == 3 { 0.0 } else { 0.5 };
                    let swing = Swing::Leg { phase, hind };
                    b.put(Role::Leg, size, hip - Vec3::Y * legs / 2.0, hip, 0, swing);
                    if hind && *haunches {
                        // A thigh over the hind leg, bulging from the body's side.
                        let thigh = Vec3::new(body.x * 0.2, body.y * 0.7, body.z * 0.42);
                        let at = Vec3::new(sx * (body.x / 2.0 - thigh.x * 0.3), legs + thigh.y * 0.42, body.z / 2.0 - thigh.z / 2.0);
                        b.put(Role::Thigh, thigh, at, hip, 0, swing);
                    }
                }
                // The neck, if any, rises from the front of the body; the head sits on it,
                // or on the body's front, its bottom at the body's top third.
                let shoulder = Vec3::new(0.0, legs + body.y * 0.7, -body.z / 2.0 + body.z * 0.06);
                let nape = match *neck {
                    Some((length, lift)) => {
                        let length = length * scale;
                        let direction = Quat::from_rotation_x(lift.to_radians()) * Vec3::NEG_Z;
                        let thickness = Vec2::new(head.x * 0.62, head.y * 0.7);
                        b.grow(Role::Neck, shoulder, direction, length + thickness.y * 0.3, thickness, shoulder, 0, Swing::Head)
                    }
                    None => Vec3::new(0.0, legs + body.y * 0.75, -body.z / 2.0),
                };
                let centre = nape + Vec3::new(0.0, head.y * 0.25, -head.z * 0.4);
                b.put(Role::Head, head, centre, shoulder, 0, Swing::Head);
                let front = centre.z - head.z / 2.0;
                if let Some(snout) = snout.map(v) {
                    b.put(Role::Snout, snout, Vec3::new(0.0, centre.y - head.y / 2.0 + snout.y / 2.0, front - snout.z / 2.0 + 0.002), shoulder, 1, Swing::Head);
                }
                if let Some((w, h)) = *ears {
                    let (w, h) = (w * scale, h * scale);
                    for side in [-1.0f32, 1.0] {
                        let base = centre + Vec3::new(side * head.x * 0.3, head.y / 2.0 - h * 0.08, head.z * 0.15);
                        let direction = Quat::from_rotation_z(-side * ear_lean.to_radians()) * Quat::from_rotation_x(0.12) * Vec3::Y;
                        b.grow(Role::Ear, base, direction, h, Vec2::new(w, w * 0.45), shoulder, 0, Swing::Ear { side });
                    }
                }
                if let Some((w, h)) = *horns {
                    let (w, h) = (w * scale, h * scale);
                    for side in [-1.0f32, 1.0] {
                        let out = Vec3::X * side;
                        let base = centre + Vec3::new(side * head.x * 0.36, head.y / 2.0 - w * 0.3, head.z * 0.08);
                        match horn_shape {
                            HornShape::Straight => {
                                b.chain(Role::Horn, base, &[(Vec3::Y + out * 0.25 + Vec3::Z * 0.2, h, w)], shoulder, FEATURE, Swing::Head);
                            }
                            HornShape::Curved => {
                                let base = base - Vec3::Y * w * 0.5;
                                let segments = [(out + Vec3::Y * 0.15, h * 0.5, w), (out * 0.5 + Vec3::Y + Vec3::NEG_Z * 0.35, h * 0.5, w * 0.8)];
                                b.chain(Role::Horn, base, &segments, shoulder, FEATURE, Swing::Head);
                            }
                            HornShape::Curl => {
                                let segments = [
                                    (Vec3::Y + Vec3::Z * 0.9 + out * 0.3, h * 0.42, w * 1.15),
                                    (Vec3::Z + out * 0.5 - Vec3::Y * 0.9, h * 0.42, w),
                                    (Vec3::NEG_Y + out * 0.45 + Vec3::NEG_Z * 0.8, h * 0.36, w * 0.85),
                                    (Vec3::NEG_Z + out * 0.2 + Vec3::Y * 0.4, h * 0.22, w * 0.7),
                                ];
                                b.chain(Role::Horn, base, &segments, shoulder, FEATURE, Swing::Head);
                            }
                            HornShape::Antlers => {
                                let beam = [(Vec3::Y + out * 0.55 + Vec3::Z * 0.35, h * 0.55, w), (Vec3::Y + out * 0.35 + Vec3::Z * 0.15, h * 0.45, w * 0.85)];
                                let fork = b.chain(Role::Horn, base, &beam[..1], shoulder, FEATURE, Swing::Head);
                                let top = b.chain(Role::Horn, fork, &beam[1..], shoulder, FEATURE, Swing::Head);
                                // Tines forward from the beam, and a fork at the top.
                                let brow = base + (fork - base) * 0.35;
                                b.grow(Role::Horn, brow, Vec3::NEG_Z + Vec3::Y * 0.5, h * 0.28, Vec2::splat(w * 0.7), shoulder, FEATURE, Swing::Head);
                                b.grow(Role::Horn, fork, Vec3::NEG_Z + Vec3::Y * 0.9, h * 0.3, Vec2::splat(w * 0.7), shoulder, FEATURE, Swing::Head);
                                b.grow(Role::Horn, top, Vec3::NEG_Z * 0.6 + Vec3::Y + out * 0.3, h * 0.2, Vec2::splat(w * 0.6), shoulder, FEATURE, Swing::Head);
                            }
                            HornShape::Tusks => {
                                let jaw = Vec3::new(side * head.x * 0.32, centre.y - head.y * 0.3, front - snout.map_or(0.0, |s| s.2 * scale) * 0.6);
                                b.chain(Role::Horn, jaw, &[(Vec3::Y + Vec3::NEG_Z * 0.5 + out * 0.4, h, w)], shoulder, FEATURE, Swing::Head);
                            }
                        }
                    }
                }
                if let Some(tail) = tail.map(v) {
                    let root = Vec3::new(0.0, legs + body.y * 0.8, body.z / 2.0 - tail.z * 0.1);
                    let direction = Quat::from_rotation_x(tail_droop.to_radians()) * Vec3::Z;
                    // The tail's length runs along its direction, its height across.
                    b.grow(Role::Tail, root, direction, tail.z, Vec2::new(tail.x, tail.y), root, 1, Swing::Tail);
                }
            }
            Model::Biped { body, head, legs, arms, wings, beak, tail, tilt, tail_lift, crest, wattle } => {
                let (body, head, legs) = (v(*body), v(*head), *legs * scale);
                let tilt = Quat::from_rotation_x(tilt.to_radians());
                let torso = Vec3::new(0.0, legs + body.y / 2.0, 0.0);
                b.parts.push(Part { size: body, centre: torso, rest: tilt, pivot: torso, colour: 0, role: Role::Torso, swing: Swing::Still });
                let birdlike = arms.is_none();
                let leg = Vec3::new((body.x * 0.25).max(0.05), legs, (body.x * 0.25).max(0.05)) * if birdlike { Vec3::new(0.55, 1.0, 0.55) } else { Vec3::ONE };
                let leg_colour = if birdlike { FEATURE } else { 1 };
                for (side, phase) in [(-1.0, 0.0), (1.0, 0.5)] {
                    let hip = Vec3::new(side * body.x * 0.25, legs + body.y * 0.1, body.z * if birdlike { 0.05 } else { 0.0 });
                    let swing = Swing::Leg { phase, hind: false };
                    let length = legs + body.y * 0.1;
                    b.put(Role::Leg, leg.with_y(length), hip - Vec3::Y * length / 2.0, hip, leg_colour, swing);
                    if birdlike {
                        let toes = Vec3::new(leg.x * 2.6, leg.x * 0.7, leg.x * 3.2);
                        b.put(Role::Foot, toes, Vec3::new(hip.x, toes.y / 2.0, hip.z - toes.z * 0.3), hip, FEATURE, swing);
                    }
                }
                let neck = torso + tilt * Vec3::new(0.0, body.y * 0.42, -body.z * if birdlike { 0.36 } else { 0.0 });
                let centre = neck + Vec3::new(0.0, head.y * 0.45, -head.z * 0.15);
                b.put(Role::Head, head, centre, neck, 0, Swing::Head);
                let front = centre.z - head.z / 2.0;
                if let Some(beak) = beak.map(v) {
                    b.put(Role::Beak, beak, Vec3::new(0.0, centre.y - head.y * 0.12, front - beak.z / 2.0 + 0.002), neck, FEATURE, Swing::Head);
                    if let Some((w, h)) = *wattle {
                        let size = Vec3::new(w * scale, h * scale, w * scale * 0.8);
                        b.put(Role::Crest, size, Vec3::new(0.0, centre.y - head.y * 0.12 - beak.y / 2.0 - size.y / 2.0, front - size.z * 0.3), neck, CREST, Swing::Head);
                    }
                }
                if let Some(crest) = crest.map(v) {
                    b.put(Role::Crest, crest, centre + Vec3::new(0.0, head.y / 2.0 + crest.y / 2.0 - 0.01, -head.z * 0.05), neck, CREST, Swing::Head);
                }
                if let Some(arms) = arms.map(|a| a * scale) {
                    let arm = Vec3::new(leg.x * 0.9, arms, leg.z * 0.9);
                    for (side, phase) in [(-1.0, 0.5), (1.0, 0.0)] {
                        let shoulder = Vec3::new(side * (body.x / 2.0 + arm.x / 2.0), legs + body.y * 0.9, 0.0);
                        b.put(Role::Arm, arm, shoulder - Vec3::Y * arms / 2.0, shoulder, 0, Swing::Leg { phase, hind: false });
                    }
                }
                if let Some((w, l)) = *wings {
                    // Folded along the sides, a little behind the middle.
                    let wing = Vec3::new(w * scale, body.y * 0.55, l * scale);
                    for side in [-1.0, 1.0] {
                        // Folded close against the upper side, the tips past the rump.
                        let shoulder = torso + tilt * Vec3::new(side * body.x / 2.0, body.y * 0.35, -body.z * 0.3);
                        let centre = torso + tilt * Vec3::new(side * (body.x / 2.0 + wing.x / 2.0 - 0.01), body.y * 0.15, -body.z * 0.3 + wing.z / 2.0);
                        b.parts.push(Part { size: wing, centre, rest: tilt, pivot: shoulder, colour: 0, role: Role::Wing, swing: Swing::Wing { side } });
                    }
                }
                if let Some(tail) = tail.map(v) {
                    let root = torso + tilt * Vec3::new(0.0, body.y * 0.15, body.z / 2.0 - tail.z * 0.1);
                    let direction = tilt * Quat::from_rotation_x(-tail_lift.to_radians()) * Vec3::Z;
                    b.grow(Role::Tail, root, direction, tail.z, Vec2::new(tail.x, tail.y), root, 1, Swing::Tail);
                }
            }
            Model::Crawler { body, head, legs, pairs } => {
                let (body, head, legs) = (v(*body), v(*head), *legs * scale);
                let height = legs * 0.5;
                let torso = Vec3::new(0.0, height + body.y / 2.0, 0.0);
                b.put(Role::Torso, body, torso, torso, 0, Swing::Still);
                let centre = Vec3::new(0.0, height + head.y / 2.0, -body.z / 2.0 - head.z / 2.0 + 0.01);
                b.put(Role::Head, head, centre, centre, 0, Swing::Head);
                let pairs = (*pairs).clamp(1, 4) as usize;
                let thickness = (legs * 0.11).max(0.04);
                for i in 0..pairs {
                    let along = (i as f32 + 0.5) / pairs as f32;
                    let z = -body.z / 2.0 + body.z * along;
                    // Legs splay forward at the front and back at the rear.
                    let splay = (along - 0.5) * 1.2;
                    for side in [-1.0f32, 1.0] {
                        let hip = Vec3::new(side * body.x * 0.45, height + body.y * 0.3, z);
                        let phase = if (i % 2 == 0) == (side > 0.0) { 0.0 } else { 0.5 };
                        // Up and out to a knee above the body's side, then down to a foot on
                        // the ground.
                        let out = Vec3::new(side, 0.0, splay).normalize();
                        let knee = hip + out * legs * 0.42 + Vec3::Y * legs * 0.28;
                        let foot = (hip + out * legs * 0.85).with_y(0.0) + Vec3::Y * thickness * 0.4;
                        b.span(Role::Leg, hip, knee, thickness, hip, 1, Swing::Scuttle { phase });
                        b.span(Role::Leg, knee, foot, thickness * 0.8, hip, 1, Swing::Scuttle { phase });
                    }
                }
            }
            Model::Blob { body, head, shell, stalks, motes } => {
                let body = v(*body);
                let torso = Vec3::new(0.0, body.y / 2.0, 0.0);
                b.put(Role::Torso, body, torso, torso, 0, Swing::Still);
                if let Some(head) = head.map(v) {
                    let centre = Vec3::new(0.0, body.y * 0.6 + head.y / 2.0, -body.z / 2.0 + head.z / 2.0);
                    b.put(Role::Head, head, centre, centre, 0, Swing::Head);
                    if let Some(length) = stalks.map(|l| l * scale) {
                        let thick = (head.x * 0.16).max(0.025);
                        for side in [-1.0f32, 1.0] {
                            let base = centre + Vec3::new(side * head.x * 0.25, head.y / 2.0 - 0.01, -head.z * 0.2);
                            let tip = b.grow(Role::Stalk, base, Vec3::Y + Vec3::X * side * 0.3 + Vec3::NEG_Z * 0.25, length, Vec2::splat(thick), centre, 0, Swing::Head);
                            let eye = Vec3::splat(thick * 1.6);
                            b.put(Role::Eye, eye, tip, centre, 0, Swing::Head);
                        }
                    }
                }
                if let Some(shell) = shell.map(v) {
                    let at = Vec3::new(0.0, body.y + shell.y / 2.0 - body.y * 0.2, body.z * 0.1);
                    b.put(Role::Shell, shell, at, torso, 1, Swing::Still);
                    // The spiral's lip, turning out of one side.
                    let lip = shell * Vec3::new(0.3, 0.55, 0.55);
                    b.put(Role::Shell, lip, at + Vec3::new(shell.x / 2.0 + lip.x / 2.0 - 0.01, -shell.y * 0.05, shell.z * 0.05), torso, 1, Swing::Still);
                }
                for i in 0..*motes {
                    let size = Vec3::splat((body.x * 0.18).max(0.04));
                    let at = torso + Vec3::new(body.x * 0.95, body.y * (0.15 * (i as f32 % 3.0) - 0.15), 0.0);
                    b.put(Role::Mote, size, at, torso, 0, Swing::Orbit { phase: i as f32 / *motes as f32 });
                }
            }
            Model::Boxes(boxes) => {
                for def in boxes {
                    let centre = v(def.at);
                    let (pitch, yaw, roll) = def.turn;
                    let rest = Quat::from_euler(EulerRot::YXZ, yaw.to_radians(), pitch.to_radians(), roll.to_radians());
                    let pivot = def.pivot.map_or(centre, v);
                    b.parts.push(Part { size: v(def.size), centre, rest, pivot, colour: def.colour, role: def.role, swing: def.swing });
                }
            }
        }
        b.parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lowest(part: &Part) -> f32 {
        let mut low = f32::MAX;
        for x in [-0.5, 0.5] {
            for y in [-0.5, 0.5] {
                for z in [-0.5, 0.5] {
                    low = low.min(part.at_rest(Vec3::new(x, y, z) * part.size).y);
                }
            }
        }
        low
    }

    #[test]
    fn plans_stand_on_the_ground() {
        let models = [
            Model::Quadruped {
                body: (0.9, 0.85, 1.45),
                head: (0.5, 0.46, 0.52),
                legs: 0.62,
                ears: Some((0.1, 0.12)),
                tail: Some((0.08, 0.08, 0.42)),
                horns: Some((0.12, 0.36)),
                horn_shape: HornShape::Curl,
                snout: Some((0.3, 0.2, 0.12)),
                neck: Some((0.3, 40.0)),
                haunches: true,
                hump: Some(0.3),
                tail_droop: 70.0,
                ear_lean: 20.0,
            },
            Model::Biped {
                body: (0.3, 0.3, 0.42),
                head: (0.2, 0.22, 0.2),
                legs: 0.2,
                arms: None,
                wings: Some((0.06, 0.32)),
                beak: Some((0.08, 0.06, 0.1)),
                tail: Some((0.08, 0.22, 0.14)),
                tilt: 15.0,
                tail_lift: 50.0,
                crest: Some((0.04, 0.08, 0.14)),
                wattle: Some((0.05, 0.06)),
            },
            Model::Crawler { body: (0.6, 0.4, 0.7), head: (0.4, 0.34, 0.36), legs: 0.6, pairs: 4 },
            Model::Blob { body: (0.3, 0.2, 0.6), head: Some((0.18, 0.18, 0.18)), shell: Some((0.36, 0.34, 0.36)), stalks: Some(0.12), motes: 0 },
        ];
        for model in &models {
            let parts = model.parts(1.0);
            let low = parts.iter().map(lowest).fold(f32::MAX, f32::min);
            assert!(low > -0.02, "{model:?} pokes {low} below the ground");
            assert!(low < 0.02, "{model:?} floats {low} above the ground");
        }
    }

    #[test]
    fn grown_boxes_run_from_their_base() {
        let mut b = Builder { parts: Vec::new() };
        let end = b.grow(Role::Horn, Vec3::ZERO, Vec3::X, 2.0, Vec2::splat(0.1), Vec3::ZERO, 2, Swing::Still);
        assert!(end.abs_diff_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5));
        let part = b.parts[0];
        assert!(part.at_rest(Vec3::new(0.0, -1.0, 0.0)).abs_diff_eq(Vec3::ZERO, 1e-5));
        assert!(part.at_rest(Vec3::new(0.0, 1.0, 0.0)).abs_diff_eq(end, 1e-5));
    }
}
