//! Anatomy: the shape of a creature's body as an animal's, not as its boxes.
//!
//! The box parts ([`super::Part`]) are the rig: what moves. Drawn as they are, rounded,
//! they read as cylinders and blobs (Mariano, 2026-10-06: "they look pretty terrible if
//! it's just a cylinder for a body"). So each body plan also gets an anatomy: a skeleton
//! of round cones (a sphere at each end, smoothly joined) laid where an animal's mass is.
//! A four-legged body has a rump and a loin, a waist tucked up under the back, a deep
//! chest with a brisket in front and withers above, shoulder blades and haunches over the
//! legs; legs bend at elbow and knee, stifle and hock, down to slim fetlocks and wider
//! hooves; the neck thickens to the shoulders; the head is a cranium, a brow running into
//! a muzzle that narrows to the nose, a jaw beneath and cheeks either side. A bird has a
//! full breast and a narrow rump, a short neck, a round head and drumsticks. The cones are
//! blended and meshed low-poly (`volume`), and the boxes still say which part each point
//! of the body moves with. Ears, horns, tails, wings, beaks and the like keep their part's
//! own shape, which carries their character.
//!
//! The anatomy is made from the same numbers as the parts (and where the parts are), so
//! every species, a mod's or a generated one, gets one without a model file.

use glam::{Quat, Vec3};

use super::model::{Model, Part, Role};

/// A round cone: a sphere of radius `ra` at `a` and one of `rb` at `b`, joined smoothly,
/// its cross-section squashed across (x) by `squash` (under 1 for a narrow body).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    pub a: Vec3,
    pub b: Vec3,
    pub ra: f32,
    pub rb: f32,
    pub squash: f32,
}

impl Cone {
    fn new(a: Vec3, ra: f32, b: Vec3, rb: f32, squash: f32) -> Self {
        Self { a, b, ra, rb, squash }
    }

    fn ball(at: Vec3, r: f32, squash: f32) -> Self {
        Self::new(at, r, at, r, squash)
    }

    /// Signed distance from `p` (Inigo Quilez's round cone, with the squash applied by
    /// stretching space across).
    pub fn distance(&self, p: Vec3) -> f32 {
        let s = Vec3::new(1.0 / self.squash, 1.0, 1.0);
        let (p, a, b) = (p * s, self.a * s, self.b * s);
        let ba = b - a;
        let l2 = ba.length_squared();
        let d = if l2 < 1e-8 {
            (p - a).length() - self.ra
        } else {
            let rr = self.ra - self.rb;
            let a2 = l2 - rr * rr;
            let il2 = 1.0 / l2;
            let pa = p - a;
            let y = pa.dot(ba);
            let z = y - l2;
            let x2 = (pa * l2 - ba * y).length_squared();
            let y2 = y * y * l2;
            let z2 = z * z * l2;
            let k = rr.signum() * rr * rr * x2;
            if z.signum() * a2 * z2 > k {
                (x2 + z2).sqrt() * il2 - self.rb
            } else if y.signum() * a2 * y2 < k {
                (x2 + y2).sqrt() * il2 - self.ra
            } else {
                ((x2 * a2 * il2).sqrt() + y * rr) * il2 - self.ra
            }
        };
        d * self.squash.min(1.0)
    }
}

/// A body's anatomy: its cones, and the parts that keep their own shape.
#[derive(Clone, Debug, Default)]
pub struct Anatomy {
    pub cones: Vec<Cone>,
    /// Indices of parts drawn as themselves on top (ears, horns, tails, wings...).
    pub keep: Vec<usize>,
}

/// The anatomy of a body plan, from its parts; none for plans drawn as their parts.
pub fn of(model: &Model, parts: &[Part]) -> Option<Anatomy> {
    match model {
        Model::Quadruped { haunches, hump, .. } => Some(quadruped(parts, *haunches, hump.unwrap_or(0.0))),
        Model::Biped { arms: None, .. } => Some(bird(parts)),
        _ => None,
    }
}

fn find(parts: &[Part], role: Role) -> Option<&Part> {
    parts.iter().find(|p| p.role == role)
}

/// The parts that keep their own shape over an anatomy that replaces `replaced`.
fn kept(parts: &[Part], replaced: &[Role]) -> Vec<usize> {
    parts.iter().enumerate().filter(|(_, p)| !replaced.contains(&p.role)).map(|(i, _)| i).collect()
}

fn quadruped(parts: &[Part], haunches: bool, hump: f32) -> Anatomy {
    let mut cones = Vec::new();
    let Some(torso) = find(parts, Role::Torso) else { return Anatomy { cones, keep: (0..parts.len()).collect() } };
    let (w, h, l) = (torso.size.x, torso.size.y, torso.size.z);
    let c = torso.centre;
    let floor = c.y - h / 2.0;
    let legs = floor.max(0.02);
    let sq = (w / h).clamp(0.55, 1.35);
    let at = |y: f32, z: f32| Vec3::new(0.0, c.y + y * h, c.z + z * l);

    // The barrel: rump and loin, a waist tucked up, a deep chest, withers and brisket.
    let rump = at(0.06, 0.36);
    let loin = at(0.1, 0.14);
    let waist = at(0.12, -0.04);
    let chest = at(-0.02, -0.27);
    let withers = at(0.28 + hump * 0.6, -0.3);
    let brisket = at(-0.26, -0.4);
    let (r_rump, r_loin, r_waist, r_chest) = (0.44 * h, 0.4 * h, 0.36 * h, 0.5 * h);
    cones.push(Cone::new(rump, r_rump, loin, r_loin, sq));
    cones.push(Cone::new(loin, r_loin, waist, r_waist, sq));
    cones.push(Cone::new(waist, r_waist, chest, r_chest, sq));
    cones.push(Cone::new(chest, r_chest * 0.7, withers, (0.26 + hump * 0.5) * h, sq * 0.85));
    cones.push(Cone::new(chest, r_chest * 0.8, brisket, 0.28 * h, sq * 0.9));

    // Legs: the leg parts' feet say where they stand.
    let thick = parts.iter().filter(|p| p.role == Role::Leg).map(|p| p.size.x.min(p.size.z) * 0.5).fold(0.0f32, f32::max).max(0.02);
    for part in parts.iter().filter(|p| p.role == Role::Leg) {
        let foot = part.pivot.with_y(0.0);
        let sx = foot.x.signum();
        let x = |f: f32| sx * w * f;
        let hind = foot.z > c.z;
        if !hind {
            // Shoulder blade, elbow, knee, fetlock, hoof.
            let blade = Vec3::new(x(0.3), c.y + 0.08 * h, chest.z - 0.02 * l);
            let elbow = Vec3::new(x(0.3), legs * 0.95, foot.z + 0.04 * l);
            let knee = Vec3::new(x(0.29), legs * 0.45, foot.z);
            let fetlock = Vec3::new(x(0.29), legs * 0.12, foot.z - 0.01 * l);
            let hoof = Vec3::new(x(0.29), thick * 0.45, foot.z - 0.03 * l);
            cones.push(Cone::new(blade, 0.28 * h, elbow, thick * 1.25, 0.8));
            cones.push(Cone::new(elbow, thick * 1.1, knee, thick * 0.8, 1.0));
            cones.push(Cone::new(knee, thick * 0.8, fetlock, thick * 0.6, 1.0));
            cones.push(Cone::new(fetlock, thick * 0.62, hoof, thick * 0.8, 1.0));
        } else {
            // Haunch, stifle, hock, fetlock, hoof: the hind leg bends back at the hock.
            let haunch_r = if haunches { 0.5 * h } else { 0.38 * h };
            let haunch = Vec3::new(x(0.24), c.y + 0.04 * h, rump.z);
            let stifle = Vec3::new(x(0.3), legs + 0.05 * h, foot.z - 0.12 * l);
            let hock = Vec3::new(x(0.28), legs * 0.42, foot.z + 0.08 * l);
            let fetlock = Vec3::new(x(0.28), legs * 0.12, foot.z + 0.02 * l);
            let hoof = Vec3::new(x(0.28), thick * 0.45, foot.z);
            cones.push(Cone::new(haunch, haunch_r, stifle, thick * 1.5, 0.75));
            cones.push(Cone::new(stifle, thick * 1.3, hock, thick * 0.75, 1.0));
            cones.push(Cone::new(hock, thick * 0.75, fetlock, thick * 0.6, 1.0));
            cones.push(Cone::new(fetlock, thick * 0.62, hoof, thick * 0.8, 1.0));
        }
    }

    // The head: cranium, brow to muzzle, nose, jaw and cheeks, in the head's own frame.
    if let Some(head) = find(parts, Role::Head) {
        let (hw, hh, hl) = (head.size.x, head.size.y, head.size.z);
        let hsq = (hw / hh).clamp(0.55, 1.3);
        let local = |x: f32, y: f32, z: f32| head.centre + head.rest * Vec3::new(x * hw, y * hh, z * hl);
        let cranium = local(0.0, 0.08, 0.18);
        cones.push(Cone::ball(cranium, 0.5 * hh, hsq));
        let (tip, tip_r) = match find(parts, Role::Snout) {
            Some(snout) => (snout.centre + snout.rest * Vec3::new(0.0, 0.05 * snout.size.y, -0.25 * snout.size.z), 0.48 * snout.size.y.min(snout.size.x)),
            None => (local(0.0, -0.1, -0.45), 0.3 * hh),
        };
        let muzzle_base = local(0.0, -0.05, -0.2);
        cones.push(Cone::new(cranium, 0.42 * hh, muzzle_base, 0.36 * hh, hsq));
        cones.push(Cone::new(muzzle_base, 0.34 * hh, tip, tip_r, hsq.min(1.1)));
        cones.push(Cone::new(local(0.0, -0.32, 0.1), 0.24 * hh, local(0.0, -0.32, -0.3), 0.16 * hh, hsq));
        for sx in [-1.0, 1.0] {
            cones.push(Cone::ball(local(sx * 0.26, -0.08, 0.0), 0.26 * hh, 1.0));
        }
        // The neck, from the top of the chest to under the cranium, thick at the base.
        let base = Vec3::new(0.0, c.y + 0.12 * h, chest.z - 0.08 * l);
        let nape = local(0.0, -0.05, 0.38);
        match find(parts, Role::Neck) {
            Some(neck) => {
                cones.push(Cone::new(base, 0.36 * h, neck.centre, 0.55 * neck.size.x.min(neck.size.z), sq * 0.9));
                cones.push(Cone::new(neck.centre, 0.5 * neck.size.x.min(neck.size.z), nape, 0.38 * hh, hsq));
            }
            None => cones.push(Cone::new(base, 0.36 * h, nape, 0.4 * hh, sq * 0.9)),
        }
    }
    Anatomy { cones, keep: kept(parts, &[Role::Torso, Role::Hump, Role::Thigh, Role::Neck, Role::Head, Role::Snout, Role::Leg]) }
}

fn bird(parts: &[Part]) -> Anatomy {
    let mut cones = Vec::new();
    let Some(torso) = find(parts, Role::Torso) else { return Anatomy { cones, keep: (0..parts.len()).collect() } };
    let (w, h, l) = (torso.size.x, torso.size.y, torso.size.z);
    let tilt: Quat = torso.rest;
    let at = |y: f32, z: f32| torso.centre + tilt * Vec3::new(0.0, y * h, z * l);
    let sq = (w / h).clamp(0.6, 1.3);
    // A full breast low in front, a narrower back and rump.
    let breast = at(-0.08, -0.22);
    let rump = at(0.06, 0.32);
    cones.push(Cone::new(breast, 0.52 * h, rump, 0.32 * h, sq));
    cones.push(Cone::ball(at(0.12, 0.0), 0.42 * h, sq));
    // Drumsticks over the legs.
    for leg in parts.iter().filter(|p| p.role == Role::Leg) {
        let hip = leg.pivot;
        let thigh = Vec3::new(hip.x * 1.1, hip.y + 0.12 * h, hip.z);
        cones.push(Cone::new(thigh, 0.24 * h, hip - Vec3::Y * 0.1 * h, 0.12 * h, 1.0));
    }
    if let Some(head) = find(parts, Role::Head) {
        let hh = head.size.y;
        let hsq = (head.size.x / hh).clamp(0.6, 1.2);
        cones.push(Cone::ball(head.centre + Vec3::new(0.0, 0.0, 0.05 * head.size.z), 0.52 * hh, hsq));
        // A short neck, thick where it meets the breast.
        let base = at(0.32, -0.3);
        cones.push(Cone::new(base, 0.3 * h, head.centre - Vec3::Y * 0.2 * hh, 0.36 * hh, sq));
    }
    Anatomy { cones, keep: kept(parts, &[Role::Torso, Role::Head]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature::model::HornShape;

    #[test]
    fn a_deer_has_a_waist_and_legs_that_reach_the_ground() {
        let model = Model::Quadruped {
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
        };
        let parts = model.parts(1.0);
        let anatomy = of(&model, &parts).expect("quadrupeds have an anatomy");
        let inside = |p: Vec3| anatomy.cones.iter().any(|c| c.distance(p) < 0.0);
        // The waist is tucked: under the back at the middle is air, under the chest is not.
        let torso = parts.iter().find(|p| p.role == Role::Torso).unwrap();
        let low = torso.centre.y - torso.size.y * 0.42;
        assert!(!inside(Vec3::new(0.0, low, torso.centre.z)), "no tuck at the waist");
        assert!(inside(Vec3::new(0.0, low, torso.centre.z - 0.3)), "the chest is not deep");
        // Hooves reach the ground under each leg.
        for leg in parts.iter().filter(|p| p.role == Role::Leg) {
            let foot = leg.pivot.with_y(0.01);
            assert!(anatomy.cones.iter().any(|c| c.distance(foot + Vec3::new(-leg.pivot.x * 0.05, 0.0, 0.0)) < 0.06), "a leg stops short of the ground");
        }
        // Ears, horns and the tail keep their own shapes.
        assert!(anatomy.keep.iter().any(|&i| parts[i].role == Role::Horn));
        assert!(!anatomy.keep.iter().any(|&i| parts[i].role == Role::Torso));
    }

    #[test]
    fn round_cones_measure_true() {
        let cone = Cone::new(Vec3::ZERO, 0.5, Vec3::new(0.0, 2.0, 0.0), 0.25, 1.0);
        assert!((cone.distance(Vec3::new(0.0, -1.0, 0.0)) - 0.5).abs() < 1e-4);
        assert!((cone.distance(Vec3::new(0.0, 3.0, 0.0)) - 0.75).abs() < 1e-4);
        assert!(cone.distance(Vec3::new(0.0, 1.0, 0.0)) < 0.0);
    }
}
