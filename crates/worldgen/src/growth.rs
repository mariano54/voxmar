//! Branching grown by space colonisation (Runions, Lane and Prusinkiewicz, "Modeling Trees
//! with a Space Colonization Algorithm", 2007; see INSPIRATIONS.md): a crown's volume is
//! seeded with points the tree competes for, and every step each branch end reaching for
//! some of them grows a block towards them, until it has taken them all. Limbs fork where
//! points pull apart, spread to fill the crown and leave sky between them where the points
//! ran out, so no two trees branch alike, yet each keeps its species' outline.
//!
//! The result is a skeleton: nodes a step apart, each knowing its parent and how many
//! twig ends it carries (the pipe model: a limb is as thick as the twigs it feeds), which
//! the tree grower turns into wood and leaves.

use voxmar_engine::glam::Vec3;

use crate::rng::Rng;

/// A crown's volume: an ellipsoid `radius` across, reaching `up` above its centre and
/// `down` below it, whose outer shell, from `hollow` (a fraction of the way out) to its
/// surface, holds the billows, as on a real tree, whose inside is limbs in shade; or a
/// cone `radius` across at its foot (the centre) narrowing to a point `up` above it.
#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    pub centre: Vec3,
    pub radius: f32,
    pub up: f32,
    pub down: f32,
    pub hollow: f32,
    pub shape: Shape,
}

#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// Points gather in `count` balls `radius` across set in the ellipsoid's shell, so the
    /// crown grows as billows of leaves on limbs with sky between them.
    Billows { count: usize, radius: f32 },
    /// Points fill the cone in thin layers `spacing` apart, so limbs grow out in tiers.
    Tiers { spacing: f32 },
}

impl Envelope {
    fn contains(&self, p: Vec3) -> bool {
        let o = p - self.centre;
        if let Shape::Tiers { .. } = self.shape {
            let t = o.y / self.up;
            let r = self.radius * (1.0 - t);
            let d = (o.x * o.x + o.z * o.z).sqrt();
            return (0.0..=1.0).contains(&t) && d <= r && d >= r * self.hollow;
        }
        let v = o.y / if o.y > 0.0 { self.up } else { self.down };
        let d = (o.x * o.x + o.z * o.z) / (self.radius * self.radius) + v * v;
        d <= 1.0 && d >= self.hollow * self.hollow
    }

    /// The volume the points fill: the billows', or the cone's tiers'.
    fn volume(&self) -> f32 {
        use std::f32::consts::PI;
        match self.shape {
            Shape::Billows { count, radius } => count as f32 * 4.0 / 3.0 * PI * radius.powi(3),
            Shape::Tiers { spacing } => PI * self.radius * self.radius * self.up / 3.0 * (1.0 - self.hollow * self.hollow) * (0.6 / spacing).min(1.0),
        }
    }
}

/// How a species branches.
#[derive(Clone, Copy, Debug)]
pub struct Habit {
    /// Points per cubic block of crown: denser crowns branch more finely.
    pub density: f32,
    /// How far a branch end feels a point, in blocks.
    pub influence: f32,
    /// A point is taken once a branch end comes this close.
    pub kill: f32,
    /// Added to every step's direction: up for trees that reach for the sky, down for
    /// drooping ones.
    pub tropism: Vec3,
}

pub const NONE: u16 = u16::MAX;

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub at: Vec3,
    /// The node this one grew from, or `NONE` for the first.
    pub parent: u16,
    /// Twig ends this node carries, itself included if it is one.
    pub tips: u16,
}

/// Most points a crown is seeded with, and most nodes it grows: a bound on the work and
/// memory of the largest giant.
const MOST_POINTS: usize = 160;
const MOST_NODES: usize = 600;
const MOST_STEPS: usize = 48;

/// Grows branches from `stem` (points a block apart up the trunk, from the crown's foot to
/// its top, every one of which branches may leave) into `envelope`. Returns the stem
/// followed by the branches, each node after its parent.
pub fn colonise(stem: &[Vec3], envelope: &Envelope, habit: &Habit, rng: &mut Rng) -> Vec<Node> {
    let mut nodes: Vec<Node> =
        stem.iter().enumerate().map(|(i, &at)| Node { at, parent: if i == 0 { NONE } else { i as u16 - 1 }, tips: 0 }).collect();
    let wanted = ((envelope.volume() * habit.density) as usize).clamp(8, MOST_POINTS);
    let mut points = Vec::with_capacity(wanted);
    let reach = Vec3::new(envelope.radius, envelope.up.max(envelope.down), envelope.radius);
    let mut unit = || Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 2.0 - 1.0;
    match envelope.shape {
        Shape::Billows { count, radius } => {
            // Billows keep apart, so sky shows between them.
            let mut centres: Vec<Vec3> = Vec::with_capacity(count);
            for _ in 0..count * 8 {
                let c = envelope.centre + unit() * reach;
                if centres.len() < count && envelope.contains(c) && centres.iter().all(|o| o.distance(c) > radius * 1.3) {
                    centres.push(c);
                }
            }
            for i in 0..wanted * 4 {
                let u = unit();
                if !centres.is_empty() && points.len() < wanted && u.length_squared() <= 1.0 {
                    points.push(centres[i % centres.len()] + u * radius);
                }
            }
        }
        Shape::Tiers { spacing } => {
            for _ in 0..wanted * 4 {
                let mut p = envelope.centre + unit() * reach;
                // Onto the nearest tier, within a fifth of a block of it.
                let tier = ((p.y - envelope.centre.y) / spacing).round() * spacing;
                p.y = envelope.centre.y + tier + unit().x * 0.2;
                if points.len() < wanted && envelope.contains(p) {
                    points.push(p);
                }
            }
        }
    }
    let (influence, kill) = (habit.influence * habit.influence, habit.kill * habit.kill);
    // Each point remembers the node nearest it. Nodes never move or go, so after each step
    // a point need only look at the nodes just grown: the whole growth costs points × nodes
    // rather than that again for every step.
    let mut nearest: Vec<(u16, f32)> = vec![(NONE, f32::INFINITY); points.len()];
    let mut grown = 0;
    let mut pull: Vec<Vec3> = Vec::new();
    for _ in 0..MOST_STEPS {
        for (p, near) in points.iter().zip(&mut nearest) {
            for (i, n) in nodes.iter().enumerate().skip(grown) {
                let d = n.at.distance_squared(*p);
                if d < near.1 {
                    *near = (i as u16, d);
                }
            }
        }
        // Each point pulls only the nearest branch end within its influence.
        pull.clear();
        pull.resize(nodes.len(), Vec3::ZERO);
        for (p, &(i, d)) in points.iter().zip(&nearest) {
            if d < influence {
                pull[i as usize] += (*p - nodes[i as usize].at) / d.sqrt().max(1e-3);
            }
        }
        grown = nodes.len();
        let mut stuck = Vec::new();
        for (i, &towards) in pull.iter().enumerate() {
            if towards == Vec3::ZERO || nodes.len() >= MOST_NODES {
                continue;
            }
            let direction = (towards.normalize_or_zero() + habit.tropism).normalize_or(Vec3::Y);
            let at = nodes[i].at + direction;
            // Points pulling a node evenly from both sides would have it grow the same twig
            // again and again: those points are given up.
            if nodes[i + 1..].iter().any(|n| n.parent == i as u16 && n.at.distance_squared(at) < 0.04) {
                stuck.push(i as u16);
                continue;
            }
            nodes.push(Node { at, parent: i as u16, tips: 0 });
        }
        if nodes.len() == grown {
            break;
        }
        let mut k = 0;
        while k < points.len() {
            let (p, (i, _)) = (points[k], nearest[k]);
            if stuck.contains(&i) || nodes[grown..].iter().any(|n| n.at.distance_squared(p) <= kill) {
                points.swap_remove(k);
                nearest.swap_remove(k);
            } else {
                k += 1;
            }
        }
        if points.is_empty() {
            break;
        }
    }
    // Twig ends count one, and every node carries the twigs of all it bears.
    let mut children = vec![false; nodes.len()];
    for n in &nodes {
        if n.parent != NONE {
            children[n.parent as usize] = true;
        }
    }
    for i in (0..nodes.len()).rev() {
        if !children[i] {
            nodes[i].tips = nodes[i].tips.max(1);
        }
        let (parent, tips) = (nodes[i].parent, nodes[i].tips);
        if parent != NONE {
            nodes[parent as usize].tips = nodes[parent as usize].tips.saturating_add(tips);
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Branches fill the crown from the stem: every node joins the tree by its parent, nearly
    /// all of them inside or near the envelope, and the stem carries every twig.
    #[test]
    fn branches_fill_the_crown() {
        let stem: Vec<Vec3> = (0..6).map(|y| Vec3::new(0.5, 4.0 + y as f32, 0.5)).collect();
        let envelope = Envelope { centre: Vec3::new(0.5, 8.0, 0.5), radius: 4.0, up: 3.0, down: 2.5, hollow: 0.5, shape: Shape::Billows { count: 6, radius: 1.8 } };
        let habit = Habit { density: 0.8, influence: 4.5, kill: 1.3, tropism: Vec3::Y * 0.2 };
        let nodes = colonise(&stem, &envelope, &habit, &mut Rng::new(1, 2, 3, 4));
        assert!(nodes.len() > 40, "{} nodes", nodes.len());
        for (i, n) in nodes.iter().enumerate().skip(1) {
            assert!((n.parent as usize) < i);
            assert!(n.at.distance(nodes[n.parent as usize].at) < 1.01);
        }
        let ends = (0..nodes.len()).filter(|&i| nodes.iter().all(|n| n.parent as usize != i)).count() as u16;
        assert_eq!(nodes[0].tips, ends);
        let wide = nodes.iter().map(|n| (n.at - envelope.centre).with_y(0.0).length()).fold(0.0, f32::max);
        assert!(wide > 2.5 && wide < 5.5, "reaches {wide}");
    }
}
