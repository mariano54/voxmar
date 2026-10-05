//! Procedural trees, in the spirit of TanTanDev's shrubbery: a species is a handful of
//! parameters (trunk height, thickness and lean, roots, and a crown shape) defined in
//! `trees.ron`, and each tree lays out its trunk, limbs and clumps of leaves in continuous
//! space from its own random stream, then voxelizes them. Randomness tied to a voxel comes
//! from hashing its position, never from the stream, so every column a tree reaches into
//! builds the same tree and keeps only its own part.
//!
//! What makes them look grown rather than stamped:
//!
//! - **Age.** Each tree draws an age: young trees in gaps and at the edges of stands,
//!   mature ones mostly, and now and then a giant half again as tall, more of them in
//!   old-growth cores. Height, crown, girth and roots all follow it, so a stand mixes
//!   sizes the way a real one does.
//! - **Grown crowns.** Broadleaf domes, conifers' cones, pines' pads and acacias' flat
//!   crowns are grown by space colonisation (`growth`): limbs branch out from the trunk
//!   towards points scattered through the crown's shape, gathered into billows on
//!   broadleaves, until they fill it, and the twigs carry clumps of leaves, flat beneath
//!   and lumpy at the edge. So no two trees branch alike, limbs show from below and
//!   between the billows, and sky shows through. Conifers' limbs droop and carry needles
//!   along their last blocks; willows hang curtains from the underside of their clumps.
//! - **Fruit.** A species may bear fruit: a share of its crown's outer leaves, most of them
//!   on its underside and rim, become a fruiting kind of its leaves (`TreeParts::fruit`).
//! - **Trunks that taper and root.** Tall thick trunks lose their corners one by one as
//!   they rise; old trees flare into roots that follow the ground, giants into buttresses.
//! - **Standing on the ground.** A trunk's foot, and every root's, reaches down through
//!   air, water and plants to the first firm block in its own block column, so nothing
//!   hangs over a dip, a pond or the lip of a cave mouth. Each column extends only what
//!   lies in it, from its own blocks, so every column still agrees.
//! - **Bushes in the ground.** Bushes are low, lumpy mounds draped over the ground from
//!   their stem's level, so on a slope they sit in it rather than hang off it.
//!
//! Some shapes follow other generators: snags' stair-stepping limbs, the willow's jog and
//! its hanging leaves are Wilder Wild's, the cypress's profile and flaring roots
//! Terrestria's, lumpy bushes Geophilic's (see INSPIRATIONS.md).

use std::f32::consts::TAU;

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::{IVec3, Vec2, Vec3};

use crate::content::{Crown, Roots, WorldContent};
use crate::growth::{self, Envelope, Habit, Shape};
use crate::geology::smoothstep;
use crate::rng::{Rng, hash3};
use crate::terrain::Blocks;

/// Furthest any part of a tree reaches from its base, horizontally.
pub const REACH: i32 = 8;
/// Furthest a trunk's foot or a root reaches down to find firm ground, in blocks.
const SINK: i32 = 8;
/// The golden angle: limbs set this far apart around a trunk never line up.
const GOLDEN: f32 = 2.399_963;
/// Salts for per-voxel randomness.
const LUMPS: u64 = 0x1c3a_5e7d;
const ROOTS: u64 = 0x2007_5a11;
const DRAPES: u64 = 0xd3a9_e5b1;
const FRUIT: u64 = 0xf5_0175;

/// A kind of tree, by its place in the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TreeId(pub u16);

struct Species {
    log: BlockId,
    /// Bark on every side, for limbs, roots and trunks lying on the ground: the wood of
    /// the log's family, or the log itself.
    wood: BlockId,
    leaves: BlockId,
    /// Leaves used instead of `leaves` on a fraction of trees (flowering azaleas).
    alternate: Option<(BlockId, f32)>,
    /// Fruiting leaves, and the share of the crown's outer leaves they take.
    fruit: Option<(BlockId, f32)>,
    roots_block: BlockId,
    height: (i32, i32),
    /// A 2×2 trunk.
    thick: bool,
    /// How far the top of the trunk drifts sideways, in blocks.
    lean: f32,
    roots: Roots,
    crown: Crown,
}

/// The blocks a tree put down inside the column being generated, so that decorations
/// (litter, fallen petals, fungi on logs) can follow the tree's own shape.
#[derive(Default)]
pub struct TreeParts {
    pub logs: Vec<IVec3>,
    pub leaves: Vec<IVec3>,
    /// Leaves bearing fruit (also in `leaves`).
    pub fruit: Vec<IVec3>,
}

pub struct Trees {
    species: Vec<Species>,
}

/// How large one tree grows, from its age.
struct Size {
    /// The trunk's height, in blocks.
    height: i32,
    /// The crown's dimensions against the species' own.
    crown: f32,
    /// A 2×2 trunk: the species' own, once the tree is old enough for it.
    thick: bool,
    /// How strongly its foot flares into roots, 0 ..= 1.
    flare: f32,
}

impl Size {
    /// Draws a tree's age and size. `vigour` (0 ..= 1, the forest's stand) favours old
    /// trees and giants, and makes young ones rarer.
    fn draw(s: &Species, vigour: f32, rng: &mut Rng) -> Self {
        let (lo, hi) = (s.height.0 as f32, s.height.1 as f32);
        // Where a mature tree falls in its species' range: the middle more often than the ends.
        let mature = lo + (hi - lo) * 0.5 * (rng.unit() + rng.unit());
        let (roll, spread) = (rng.unit(), rng.unit());
        // Snags are what is left of grown trees.
        let young = if matches!(s.crown, Crown::Dead { .. }) { 0.04 } else { 0.1 + 0.25 * (1.0 - vigour) };
        let giant = 0.015 + 0.045 * vigour;
        let age = if roll < young {
            lerp((0.45, 0.8), spread)
        } else if roll > 1.0 - giant {
            lerp((1.25, 1.5), spread)
        } else {
            lerp((0.84, 1.1), spread) + 0.06 * vigour
        };
        Self {
            height: ((mature * age).round() as i32).clamp(1, 48),
            crown: age.powf(0.75),
            thick: s.thick && age > 0.72,
            flare: smoothstep(1.0, 1.45, age),
        }
    }
}

fn lerp((lo, hi): (f32, f32), t: f32) -> f32 {
    lo + (hi - lo) * t
}

fn pick(rng: &mut Rng, range: (f32, f32)) -> f32 {
    lerp(range, rng.unit())
}

fn count(rng: &mut Rng, (lo, hi): (i32, i32)) -> i32 {
    rng.range(lo, hi + 1)
}

/// A horizontal unit vector at `angle`.
fn heading(angle: f32) -> Vec3 {
    Vec3::new(angle.cos(), 0.0, angle.sin())
}

impl Trees {
    pub fn new(content: &WorldContent, registry: &BlockRegistry) -> Self {
        let species = content
            .trees
            .iter()
            .map(|def| {
                let log = registry.expect(&def.log);
                let wood = def.log.strip_suffix("_log").and_then(|stem| registry.id(&format!("{stem}_wood"))).unwrap_or(log);
                Species {
                    log,
                    wood,
                    // A tree without leaves never places them.
                    leaves: def.leaves.as_deref().map_or(BlockId::AIR, |name| registry.expect(name)),
                    alternate: def.alternate.as_ref().map(|(name, share)| (registry.expect(name), *share)),
                    fruit: def.fruit.as_ref().map(|f| (registry.expect(&f.leaves), f.share)),
                    roots_block: def.root_block.as_deref().map_or(wood, |name| registry.expect(name)),
                    height: def.height,
                    thick: def.thick,
                    lean: def.lean,
                    roots: def.roots,
                    crown: def.crown,
                }
            })
            .collect();
        Self { species }
    }

    /// The leaves that colour a species' canopy from afar.
    pub fn leaves(&self, tree: TreeId) -> BlockId {
        self.species[tree.0 as usize].leaves
    }

    /// A species' upright log.
    pub fn log(&self, tree: TreeId) -> BlockId {
        self.species[tree.0 as usize].log
    }

    /// A species' wood with bark on every side, for trunks lying down.
    pub fn wood(&self, tree: TreeId) -> BlockId {
        self.species[tree.0 as usize].wood
    }

    /// Whether a species grows a 2×2 trunk.
    pub fn thick(&self, tree: TreeId) -> bool {
        self.species[tree.0 as usize].thick
    }

    /// Grows a tree standing on the ground at `base` (the first block above it), and
    /// records in `parts` the blocks it put down in the column. `vigour` (0 ..= 1, the
    /// forest's stand) favours old trees and giants: old-growth trees are taller than
    /// those at a scrubby edge. `ground` gives the height of the top ground block at any
    /// (x, z), from the analytic terrain, for roots and bushes to follow.
    #[allow(clippy::too_many_arguments)]
    pub fn grow(
        &self,
        tree: TreeId,
        base: IVec3,
        vigour: f32,
        ground: &dyn Fn(i32, i32) -> i32,
        rng: &mut Rng,
        blocks: &mut Blocks<'_>,
        parts: &mut TreeParts,
    ) {
        let s = &self.species[tree.0 as usize];
        parts.logs.clear();
        parts.leaves.clear();
        parts.fruit.clear();
        let mut g = Grower { blocks, seed: rng.next_u64(), base, parts, ground };
        let leaves = match s.alternate {
            Some((alternate, share)) if rng.chance(share) => alternate,
            _ => s.leaves,
        };
        let size = Size::draw(s, vigour, rng);
        let (height, k) = (size.height, size.crown);
        let dead = matches!(s.crown, Crown::Dead { .. });
        // A snag often stands broken off, the rest of its trunk lying beside it.
        let (height, fallen) = match s.crown {
            Crown::Dead { broken, .. } if height >= 4 && rng.chance(broken) => {
                let standing = ((height as f32 * pick(rng, (0.55, 0.85))).round() as i32).max(3);
                (standing, height - standing)
            }
            _ => (height, 0),
        };
        if let Crown::Bush { radius } = s.crown {
            let r = pick(rng, radius) * k.sqrt();
            g.mound(r, leaves, s.log, rng);
            return;
        }

        // The trunk drifts sideways along a gentle curve; `trunk(t)` is its centre line.
        // Willows jog a block sideways partway up, where their trunk forks (after Wilder
        // Wild's willows).
        let girth = if size.thick { 0.5 } else { 0.0 };
        let root = Vec3::new(base.x as f32 + 0.5 + girth, base.y as f32, base.z as f32 + 0.5 + girth);
        let angle = rng.unit() * TAU;
        // Snags mostly stand straight, and now and then lean hard.
        let tilt = if dead && rng.chance(0.3) { pick(rng, (0.7, 1.0)) } else if dead { 0.3 * rng.unit() } else { rng.unit() };
        let lean = heading(angle) * s.lean * tilt;
        let (fork, jog) = match s.crown {
            Crown::Weeping { .. } => (pick(rng, (0.35, 0.55)), [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z][rng.below(4) as usize].as_vec3()),
            _ => (1.0, Vec3::ZERO),
        };
        let lift = if s.roots == Roots::Stilts { 2.0 } else { 0.0 };
        let trunk = |t: f32| root + Vec3::Y * (lift + height as f32 * t) + lean * t * t + jog * smoothstep(fork - 0.05, fork + 0.05, t);
        let top = trunk(1.0);
        // A point inside the trunk's block a fraction `t` of the way up, where limbs start,
        // so they always join it.
        let on_trunk = |t: f32| trunk((t * height as f32).floor().clamp(0.0, (height - 1) as f32) / height as f32);
        // Tall thick trunks taper: three corners end at heights of their own, the fourth
        // carries on to the top.
        let tapers = matches!(s.crown, Crown::Round { .. } | Crown::Conical { .. } | Crown::Pine { .. } | Crown::Spire { .. });
        let mut ends = [height; 4];
        if size.thick && tapers {
            for end in &mut ends[1..] {
                *end = (height as f32 * pick(rng, (0.45, 0.85))) as i32;
            }
        }
        // Trees on stilts stand on their roots; others reach down to the ground.
        let feet = g.trunk(&trunk, height, size.thick, ends, s.log, s.roots != Roots::Stilts);

        match s.roots {
            // Thick trunks always flare a little.
            // Thick trunks always flare a little, and so do grown snags, whose roots the
            // weather has bared.
            Roots::None => {
                let flare = if size.thick || (dead && k > 0.9) { size.flare.max(0.45) } else { size.flare };
                g.flare(&feet, flare, s.wood);
            }
            Roots::Buttress => {
                let near = g.blocks.near(base.x, base.z, 5);
                let n = rng.range(4, 7);
                let phase = rng.unit() * TAU;
                for i in 0..n {
                    let out = heading(phase + i as f32 * TAU / n as f32 + 0.4 * rng.unit());
                    let from = root + out * (0.5 + girth) + Vec3::Y * pick(rng, (2.5, 4.5)) * k;
                    let mut foot = root + out * (girth + pick(rng, (2.0, 3.2)) * k.sqrt());
                    if near {
                        foot.y = (g.ground)(foot.x.floor() as i32, foot.z.floor() as i32) as f32 + 1.0;
                        g.line(from, foot, s.roots_block);
                        g.sink(foot.floor().as_ivec3() - IVec3::Y, s.roots_block);
                    }
                }
                g.flare(&feet, 1.0, s.wood);
            }
            Roots::Stilts => {
                let near = g.blocks.near(base.x, base.z, 4);
                for _ in 0..count(rng, (4, 6)) {
                    let out = heading(rng.unit() * TAU);
                    let from = root + Vec3::Y * pick(rng, (2.5, 4.0));
                    let mut foot = root + out * pick(rng, (1.8, 3.0));
                    if near {
                        foot.y = (g.ground)(foot.x.floor() as i32, foot.z.floor() as i32) as f32 + 1.0;
                        let knee = (from + foot) * 0.5 + out * 0.8 + Vec3::Y * 0.8;
                        g.line(from, knee, s.roots_block);
                        g.line(knee, foot, s.roots_block);
                        g.sink(foot.floor().as_ivec3() - IVec3::Y, s.roots_block);
                    }
                }
            }
        }

        let phase = rng.unit() * TAU;
        match s.crown {
            Crown::Round { reach, radius, squash, start } => {
                // A dome as wide as the limbs reach plus the billows at their ends, from the
                // crown's foot to above the trunk's top, its leaves in billows at the
                // twig ends.
                let wide = (pick(rng, reach) + pick(rng, radius)) * k;
                let foot = (start * height as f32).max(1.0);
                let span = height as f32 + pick(rng, radius) * k * squash - foot;
                let foliage = Foliage { radius: (1.5 * k.sqrt()).max(1.1), squash: squash.max(0.8), twigs: 1 };
                let envelope = Envelope {
                    centre: root + lean + Vec3::Y * (lift + foot + span * 0.55),
                    radius: wide,
                    up: span * 0.45 + 0.5,
                    down: span * 0.55,
                    hollow: 0.45,
                    shape: Shape::Billows { count: (3.0 + 4.0 * k * k) as usize, radius: 1.9 * k.sqrt() },
                };
                let habit = Habit { density: 0.35, influence: 7.0, kill: 1.6, tropism: Vec3::Y * 0.25 };
                g.colonised(&trunk, height, foot, &envelope, &habit, &foliage, s.wood, leaves, rng);
            }
            Crown::Conical { radius, start } => {
                // A cone from the crown's foot to a spike over the trunk, its limbs drooping
                // as they reach out and carrying needles along their last few blocks.
                let foot = (start * height as f32).max(1.0);
                let foliage = Foliage { radius: 1.4, squash: 0.5, twigs: 3 };
                let envelope = Envelope {
                    centre: root + lean + Vec3::Y * (lift + foot),
                    radius: pick(rng, radius) * k,
                    up: height as f32 + 1.5 - foot,
                    down: 0.0,
                    hollow: 0.0,
                    shape: Shape::Cone,
                };
                let habit = Habit { density: 0.9, influence: 3.5, kill: 1.2, tropism: Vec3::NEG_Y * 0.3 };
                g.colonised(&trunk, height, foot, &envelope, &habit, &foliage, s.wood, leaves, rng);
                // The leader: a spike of needles over the top.
                let tip = trunk(1.0).floor().as_ivec3();
                for dy in 0..=2 {
                    g.leaf(tip + IVec3::Y * dy, leaves);
                }
            }
            Crown::Pine { radius, start } => {
                // A Scots pine: a tall clean trunk under a high crown of a few flat pads of
                // needles on limbs that sweep up and out, with sky between the pads, and
                // stubs of dead limbs on the trunk below, where the lower crown died off.
                let foot = (start * height as f32).max(1.0);
                let foliage = Foliage { radius: 1.6, squash: 0.45, twigs: 1 };
                let envelope = Envelope {
                    centre: root + lean + Vec3::Y * (lift + height as f32 - 1.0),
                    radius: pick(rng, radius) * k,
                    up: 2.5,
                    down: (height as f32 - foot) * 0.6,
                    hollow: 0.3,
                    shape: Shape::Billows { count: count(rng, (4, 6)) as usize, radius: 1.6 * k.sqrt() },
                };
                let habit = Habit { density: 0.4, influence: 6.0, kill: 1.6, tropism: Vec3::Y * 0.35 };
                g.colonised(&trunk, height, foot, &envelope, &habit, &foliage, s.wood, leaves, rng);
                for _ in 0..rng.range(2, 5) {
                    let from = on_trunk(pick(rng, (0.3, start)));
                    let out = heading(rng.unit() * TAU);
                    g.line(from, from + out * pick(rng, (0.8, 1.6)) + Vec3::Y * 0.3, s.wood);
                }
            }
            Crown::Spire { radius, start } => {
                // Radius along the crown (Terrestria's cypress): 0 at its foot, widest a
                // third of the way up, 0 at its tip, which stands a third of the trunk above
                // the trunk's top. Its sprays twist slowly round it, so it reads as foliage
                // rather than a cone.
                let r = pick(rng, radius) * k;
                let first = (height as f32 * start) as i32;
                let last = height + height / 3;
                let span = (last + 1 - first) as f32;
                let lobes = rng.range(2, 4) as f32;
                for y in first..=last {
                    let x = (y - first) as f32 / span + 0.5 / span;
                    let width = r * 6.25 * x * (1.0 - x) * (1.0 - x);
                    let c = trunk((y as f32 / height as f32).min(1.0));
                    let c = Vec3::new(c.x, root.y + lift + y as f32 + 0.5, c.z);
                    g.whorl(c, width.max(0.7), lobes, phase + 0.45 * y as f32, 0.3, leaves);
                }
            }
            Crown::Cap { radius } => {
                // On the stem's last block, wherever its lean took it.
                let stem = trunk((height - 1) as f32 / height as f32).floor();
                g.cap(stem + Vec3::new(0.5, 1.0, 0.5), pick(rng, radius) * k.sqrt(), leaves);
            }
            Crown::Umbrella { reach, radius, squash, fork } => {
                // A wide, flat crown of billows on limbs that fork off the upper trunk and
                // spread.
                let foot = (fork * height as f32).max(1.0);
                let foliage = Foliage { radius: (1.5 * k.sqrt()).max(1.1), squash: squash.clamp(0.5, 0.8), twigs: 1 };
                let envelope = Envelope {
                    centre: root + lean + Vec3::Y * (lift + height as f32 + 1.0),
                    radius: (pick(rng, reach) + pick(rng, radius)) * k,
                    up: (pick(rng, radius) * squash * k).max(1.2),
                    down: 1.5,
                    hollow: 0.4,
                    shape: Shape::Billows { count: (3.0 + 3.0 * k * k) as usize, radius: 1.7 * k.sqrt() },
                };
                let habit = Habit { density: 0.35, influence: 7.0, kill: 1.6, tropism: Vec3::Y * 0.2 };
                g.colonised(&trunk, height, foot, &envelope, &habit, &foliage, s.wood, leaves, rng);
            }
            Crown::Weeping { radius, drape } => {
                // Short limbs under clumps, a round top, and curtains hanging from the
                // underside of all of it.
                // Limbs arch out from the upper trunk, rising and then bowing down at
                // their ends, under small clumps; the crown is a low dome over them.
                let r = pick(rng, radius) * k;
                let n = count(rng, (4, 6));
                for i in 0..n {
                    let f = (i as f32 + rng.unit()) / n as f32;
                    let from = on_trunk(lerp((0.55, 0.95), f));
                    let out = heading(phase + i as f32 * GOLDEN);
                    let length = pick(rng, (1.8, 3.0)) * k;
                    let crest = from + out * length * 0.55 + Vec3::Y * pick(rng, (0.8, 1.6));
                    let end = from + out * length + Vec3::Y * 0.3;
                    g.line(from, crest, s.wood);
                    g.line(crest, end, s.wood);
                    let r = r * pick(rng, (0.45, 0.6));
                    g.clump(crest, r, r * 0.6, r * 0.5, leaves);
                }
                g.clump(top + Vec3::Y * 0.2, r * 0.8, r * 0.55, r * 0.4, leaves);
                let low = (top.y - r - 2.0) as i32;
                g.curtains(Vec2::new(top.x, top.z), r + 2.5 * k, low, top.y as i32 + 3, drape, leaves);
            }
            Crown::Palm { fronds, length } => {
                // Fronds arch up and out from the crown, which sits on the trunk's last
                // block, then droop towards their tips.
                let top = on_trunk(1.0) + Vec3::Y * 0.5;
                let n = count(rng, fronds);
                for i in 0..n {
                    let out = heading(phase + (i as f32 + 0.3 * rng.unit()) * TAU / n as f32);
                    let l = pick(rng, length) * k.sqrt();
                    let droop = pick(rng, (0.8, 2.0));
                    let mut last = top + Vec3::Y * 0.5;
                    for step in 1..=6 {
                        let s = step as f32 / 6.0;
                        let p = top + Vec3::Y * 0.5 + out * l * s + Vec3::Y * (1.3 * (s * 2.4).sin() - droop * s * s);
                        g.line_of(last, p, leaves);
                        last = p;
                    }
                }
                g.clump(top + Vec3::Y * 0.5, 1.2, 0.7, 0.4, leaves);
            }
            Crown::Bush { .. } => unreachable!("bushes grow as mounds"),
            Crown::Dead { branches, reach, fork, .. } => {
                // Limbs climb the trunk at the golden angle, longer lower down, reaching
                // out more than up to an elbow, where most split into two twigs that
                // spread and climb in irregular stairs (after Wilder Wild's junipers), so
                // the snag keeps the spreading outline of the tree it was. Below them,
                // stubs of limbs long since snapped off.
                // Fewer on young snags, and on broken ones, whose top took its limbs down.
                let kept = height as f32 / (height + fallen) as f32;
                let n = (count(rng, branches) as f32 * k.powf(1.5) * kept).round() as i32;
                for i in 0..n {
                    let f = (i as f32 + rng.unit()) / n as f32;
                    let from = on_trunk(lerp((0.45, 0.95), f));
                    let angle = phase + i as f32 * GOLDEN + 0.4 * (rng.unit() - 0.5);
                    let length = pick(rng, reach) * k * (1.15 - 0.4 * f);
                    // The elbow stands clear of the trunk, so twigs rising from it never run
                    // up beside the trunk like a candelabrum's arms.
                    let elbow = from + heading(angle) * (length * 0.6).max(1.6) + Vec3::Y * length * pick(rng, (0.25, 0.5));
                    g.line(from, elbow, s.wood);
                    let at = elbow.floor().as_ivec3();
                    let twig = |angle: f32, length: f32, rise: f32| {
                        let up = (length * rise).round().max(1.0) as i32;
                        at + IVec3::new((angle.cos() * length).round() as i32, up, (angle.sin() * length).round() as i32)
                    };
                    if rng.chance(fork) {
                        for side in [-1.0, 1.0] {
                            let (turn, long, rise) = (pick(rng, (0.45, 0.9)), pick(rng, (0.4, 0.7)), pick(rng, (0.3, 0.7)));
                            g.walk(at, twig(angle + side * turn, (length * long).max(2.0), rise), s.wood);
                        }
                    } else {
                        let (turn, rise) = (0.5 * (rng.unit() - 0.5), pick(rng, (0.3, 0.7)));
                        g.walk(at, twig(angle + turn, (length * 0.5).max(2.0), rise), s.wood);
                    }
                }
                for _ in 0..rng.range(0, 4) {
                    let from = on_trunk(pick(rng, (0.2, 0.6)));
                    g.line(from, from + heading(rng.unit() * TAU) * pick(rng, (1.0, 1.8)), s.wood);
                }
                // What broke off lies along the ground beside the trunk, a block or two
                // out from it.
                if fallen >= 2 {
                    let along = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z][rng.below(4) as usize];
                    g.fallen(base + along * 2, along, fallen.min(6), s.wood);
                }
            }
            Crown::Bare => {}
        }
        if let Some((fruit, share)) = s.fruit {
            g.fruit(leaves, fruit, share);
        }
    }
}

/// A bush at `base` (the first block above the ground), a low, lumpy mound of `leaves`
/// about `radius` across on a stem of `stem` (none if air), draped over the ground so it
/// sits in it. Its parts go into `parts`.
#[allow(clippy::too_many_arguments)]
pub fn bush(
    base: IVec3,
    radius: f32,
    leaves: BlockId,
    stem: BlockId,
    ground: &dyn Fn(i32, i32) -> i32,
    rng: &mut Rng,
    blocks: &mut Blocks<'_>,
    parts: &mut TreeParts,
) {
    parts.logs.clear();
    parts.leaves.clear();
    parts.fruit.clear();
    let mut g = Grower { blocks, seed: rng.next_u64(), base, parts, ground };
    g.mound(radius, leaves, stem, rng);
}

/// How a grown crown carries its leaves: a clump `radius` across, flattened by `squash`,
/// on every node bearing at most `twigs` twig ends (1: at the twig ends alone, as on a
/// broadleaf; more: along the last blocks of each limb, as a conifer's needles).
struct Foliage {
    radius: f32,
    squash: f32,
    twigs: u16,
}

/// A clump of leaves: its centre, its radius across, and how far it reaches above and
/// below its centre, all to the outer edges of its blocks.
struct Clump {
    c: Vec3,
    r: f32,
    up: f32,
    down: f32,
}

/// One layer of a conifer's tier: its centre, how far its sprays reach, how many there are,
/// how they are turned and how deeply they part.
struct Whorl {
    c: Vec2,
    r: f32,
    lobes: f32,
    turn: f32,
    depth: f32,
}

/// The cells from `from` (exclusive, unless equal to `to`) to `to`, changing one axis at
/// a time (horizontal first), so consecutive cells share a face and wood never looks
/// broken where it runs diagonally.
fn connect(from: IVec3, to: IVec3) -> impl Iterator<Item = IVec3> {
    let mut at = from;
    let mut first = from == to;
    std::iter::from_fn(move || {
        if std::mem::take(&mut first) {
            return Some(at);
        }
        let axis = [0, 2, 1].into_iter().find(|&axis| at[axis] != to[axis])?;
        at[axis] += (to[axis] - at[axis]).signum();
        Some(at)
    })
}

/// Writes a tree's wood and leaves, clipped to its reach and to the column being
/// generated. Wood replaces leaves but leaves never replace wood, so a tree's parts can be
/// written in any order; overlapping trees are written in the same order by every column.
struct Grower<'a, 'b> {
    blocks: &'a mut Blocks<'b>,
    /// Seeds per-voxel randomness (ragged leaf edges, roots).
    seed: u64,
    base: IVec3,
    parts: &'a mut TreeParts,
    /// The analytic ground's height at (x, z).
    ground: &'a dyn Fn(i32, i32) -> i32,
}

impl Grower<'_, '_> {
    fn unit(&self, cell: IVec3) -> f32 {
        (hash3(self.seed, cell.x, cell.y, cell.z) & 0xffff) as f32 / 65536.0
    }

    fn unit_of(&self, salt: u64, cell: IVec3) -> f32 {
        (hash3(self.seed ^ salt, cell.x, cell.y, cell.z) & 0xffff) as f32 / 65536.0
    }

    /// A value per two-block cell of a lattice offset per tree, for bulges and bites in
    /// the outline of clumps.
    fn lump(&self, cell: IVec3) -> f32 {
        let shift = IVec3::new(self.seed as i32 & 1, (self.seed >> 1) as i32 & 1, (self.seed >> 2) as i32 & 1);
        let c = (cell + shift).div_euclid(IVec3::splat(2));
        self.unit_of(LUMPS, c)
    }

    fn reachable(&self, cell: IVec3) -> bool {
        (cell.x - self.base.x).abs() <= REACH && (cell.z - self.base.z).abs() <= REACH
    }

    /// How far a clump centred at `c` may spread and stay within reach of the trunk.
    fn room(&self, c: Vec3) -> f32 {
        let o = c - Vec3::new(self.base.x as f32 + 0.5, 0.0, self.base.z as f32 + 0.5);
        REACH as f32 + 0.5 - o.x.abs().max(o.z.abs())
    }

    fn wood(&mut self, cell: IVec3, log: BlockId) {
        if self.reachable(cell) && self.blocks.put_wood(cell, log) {
            self.parts.logs.push(cell);
        }
    }

    fn leaf(&mut self, cell: IVec3, leaves: BlockId) {
        if self.reachable(cell) {
            self.put_leaf(cell, leaves);
        }
    }

    /// Puts a leaf at a cell already known to be within reach; whether it did.
    fn put_leaf(&mut self, cell: IVec3, leaves: BlockId) -> bool {
        let put = !leaves.is_air() && self.blocks.put_if_air(cell, leaves);
        if put {
            self.parts.leaves.push(cell);
        }
        put
    }

    /// Continues wood down from `cell` through air, water, leaves and small plants to the
    /// first firm block, at most `SINK` blocks. Only the column holding `cell` writes it,
    /// from its own blocks, so every column agrees.
    fn sink(&mut self, cell: IVec3, log: BlockId) {
        if !self.reachable(cell) || !self.blocks.contains(cell) {
            return;
        }
        for d in 0..SINK {
            let at = cell - IVec3::Y * d;
            if !self.blocks.put_log(at, log) {
                return;
            }
            self.parts.logs.push(at);
        }
    }

    /// A trunk `length` blocks long lying on the ground from `start` along `along`,
    /// following the ground a block up or down at a time, and stopping where it would have
    /// to climb or drop further. A block goes down only on firm ground in its own column,
    /// so a block over a hole is left out rather than hung over it.
    fn fallen(&mut self, start: IVec3, along: IVec3, length: i32, wood: BlockId) {
        if !self.blocks.near(start.x, start.z, length + 1) {
            return;
        }
        let mut last = None;
        for j in 0..length {
            let at = start + along * j;
            let cell = at.with_y((self.ground)(at.x, at.z) + 1);
            if last.is_some_and(|y: i32| (y - cell.y).abs() > 1) {
                return;
            }
            last = Some(cell.y);
            if self.reachable(cell) && self.blocks.earth(cell - IVec3::Y) && self.blocks.put_log(cell, wood) {
                self.parts.logs.push(cell);
            }
        }
    }

    /// Logs along the trunk's centre line `centre(0..1)`, 2×2 if `thick` with each corner
    /// ending `ends` blocks up, and returns its foot: the cells it stands on the ground
    /// with. A `sunk` trunk reaches down from its foot to the ground.
    fn trunk(&mut self, centre: &impl Fn(f32) -> Vec3, height: i32, thick: bool, ends: [i32; 4], log: BlockId, sunk: bool) -> Vec<IVec3> {
        let offsets: &[(i32, i32)] = if thick { &[(0, 0), (-1, 0), (0, -1), (-1, -1)] } else { &[(0, 0)] };
        let floor = centre(0.0).y.floor() as i32;
        let mut last: Option<IVec3> = None;
        let mut feet = Vec::with_capacity(offsets.len());
        for k in 0..height {
            let cell = centre(k as f32 / height as f32).floor().as_ivec3();
            for step in connect(last.unwrap_or(cell), cell) {
                for (i, &(dx, dz)) in offsets.iter().enumerate() {
                    if step.y - floor < ends[i] {
                        self.wood(step + IVec3::new(dx, 0, dz), log);
                    }
                }
            }
            if last.is_none() {
                feet.extend(offsets.iter().map(|&(dx, dz)| cell + IVec3::new(dx, 0, dz)));
            }
            last = Some(cell);
        }
        if sunk {
            for &foot in &feet {
                self.sink(foot - IVec3::Y, log);
            }
        }
        feet
    }

    /// Roots flaring from the trunk's `feet` (after Terrestria's mega trunks): on some
    /// sides one to three blocks out along the ground, the first a block taller beside the trunk on old trees, each reaching
    /// down to the ground. `strength` (0 ..= 1) is how many sides, and how far.
    fn flare(&mut self, feet: &[IVec3], strength: f32, wood: BlockId) {
        // Roots reach three blocks from the foot; the ground is only asked for near the
        // column.
        if strength <= 0.0 || !feet.iter().any(|f| self.blocks.near(f.x, f.z, 3)) {
            return;
        }
        for &foot in feet {
            for d in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
                let next = foot + d;
                if feet.contains(&next) || self.unit_of(ROOTS, next) >= 0.75 * strength {
                    continue;
                }
                let u = self.unit_of(ROOTS ^ 1, next);
                let long = 1 + i32::from(u < 0.7 * strength) + i32::from(u < 0.25 * strength);
                for k in 1..=long {
                    let cell = foot + d * k;
                    let y = (self.ground)(cell.x, cell.z) + 1;
                    if (y - foot.y).abs() > 1 {
                        break;
                    }
                    let tall = if k == 1 && u < 0.5 * strength { 2 } else { 1 };
                    for h in 0..tall {
                        self.wood(cell.with_y(y + h), wood);
                    }
                    self.sink(cell.with_y(y - 1), wood);
                }
            }
        }
    }

    fn line_cells(from: Vec3, to: Vec3) -> Vec<IVec3> {
        let steps = ((to - from).length() * 2.0).ceil().max(1.0) as i32;
        let mut cells = vec![from.floor().as_ivec3()];
        for i in 1..=steps {
            let cell = from.lerp(to, i as f32 / steps as f32).floor().as_ivec3();
            let last = cells[cells.len() - 1];
            if cell != last {
                cells.extend(connect(last, cell));
            }
        }
        cells
    }

    fn line(&mut self, from: Vec3, to: Vec3, log: BlockId) {
        for cell in Self::line_cells(from, to) {
            self.wood(cell, log);
        }
    }

    /// A line of leaves: fronds.
    fn line_of(&mut self, from: Vec3, to: Vec3, leaves: BlockId) {
        for cell in Self::line_cells(from, to) {
            self.leaf(cell, leaves);
        }
    }

    /// Wood stepping from `from` to `to` one block at a time, upwards with a chance that
    /// grows with how much of the way left is up (after Wilder Wild's juniper branches), so
    /// limbs climb in irregular stairs rather than straight lines.
    fn walk(&mut self, from: IVec3, to: IVec3, log: BlockId) {
        let mut at = from;
        while at != to {
            let left = to - at;
            let manhattan = left.abs().element_sum();
            let up = self.unit_of(ROOTS ^ 2, at) * (manhattan as f32) < left.y.abs() as f32;
            let axis = if up {
                1
            } else if left.x.abs() >= left.z.abs() && left.x != 0 {
                0
            } else if left.z != 0 {
                2
            } else {
                1
            };
            at[axis] += left[axis].signum();
            self.wood(at, log);
        }
    }

    /// Swaps a `share` of the crown's outer `leaves` in this column for `fruit`: twice as
    /// many on its underside as on its rim, where fruit hangs and can be reached, and none
    /// on top or inside. Each leaf decides from a hash of its cell, so every column agrees.
    fn fruit(&mut self, leaves: BlockId, fruit: BlockId, share: f32) {
        for i in 0..self.parts.leaves.len() {
            let cell = self.parts.leaves[i];
            if self.blocks.get(cell) != leaves {
                continue;
            }
            // Beyond the column counts as closed, so a column's leaves decide from their own
            // blocks alone.
            let open = |d: IVec3| self.blocks.contains(cell + d) && self.blocks.get(cell + d).is_air();
            let weight = if open(IVec3::NEG_Y) {
                1.6
            } else if [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z].into_iter().any(open) {
                0.8
            } else {
                0.0
            };
            if self.unit_of(FRUIT, cell) < share * weight {
                self.blocks.set(cell, fruit);
                self.parts.fruit.push(cell);
            }
        }
    }

    /// A crown grown by space colonisation (see `growth`) from the trunk `centre(0..1)`,
    /// which is `height` blocks tall, from `foot` blocks up into `envelope`: every limb is
    /// wood, and the twigs hold clumps of leaves as `foliage` says, so the crown is the
    /// union of its twigs' clumps with sky where no twig reached.
    #[allow(clippy::too_many_arguments)]
    fn colonised(&mut self, centre: &impl Fn(f32) -> Vec3, height: i32, foot: f32, envelope: &Envelope, habit: &Habit, foliage: &Foliage, wood: BlockId, leaves: BlockId, rng: &mut Rng) {
        let envelope = Envelope { radius: envelope.radius.min(REACH as f32 - 1.0), ..*envelope };
        // Limbs may leave the trunk from any of its blocks within the crown.
        let stem: Vec<Vec3> = (foot as i32..height).map(|y| centre(y as f32 / height as f32) + Vec3::Y * 0.5).collect();
        let nodes = growth::colonise(&stem, &envelope, habit, rng);
        let (r, squash) = (foliage.radius, foliage.squash);
        for (i, n) in nodes.iter().enumerate().skip(stem.len()) {
            self.line(nodes[n.parent as usize].at, n.at, wood);
            if n.tips <= foliage.twigs && i > stem.len() {
                self.clump(n.at + Vec3::Y * 0.4, r, r * squash * 0.8, r * squash * 0.45, leaves);
            }
        }
    }

    /// A clump of leaves around `c`: `r` across, reaching `up` above its centre and `down`
    /// below, so it is flatter beneath; its outline bulges and bites at the scale of a
    /// couple of blocks, with a few leaves missing from its surface, and no leaf of it is
    /// cut off from the rest (see `in_clump`).
    fn clump(&mut self, c: Vec3, r: f32, up: f32, down: f32, leaves: BlockId) {
        // Sizes reach the edges of the outermost blocks, not their middles.
        let r = r.min(self.room(c)).max(0.7) + 0.5;
        let shape = Clump { c, r, up: up.max(0.6) + 0.5, down: down.max(0.5) + 0.5 };
        let min = Vec3::new(c.x - r, c.y - shape.down, c.z - r).floor().as_ivec3();
        let max = Vec3::new(c.x + r, c.y + shape.up, c.z + r).floor().as_ivec3();
        let Some((min, max)) = self.blocks.clip(min, max) else { return };
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    let cell = IVec3::new(x, y, z);
                    if self.reachable(cell) && self.in_clump(&shape, cell) {
                        self.put_leaf(cell, leaves);
                    }
                }
            }
        }
    }

    /// Whether `cell` is a leaf of a clump: inside its lumpy outline, and so is every cell
    /// on its way in to the clump's solid core, stepping one block at a time along the
    /// axis it lies furthest out on. So the bites in the outline never leave a leaf
    /// floating on its own, and the answer depends on nothing but the cell.
    fn in_clump(&self, s: &Clump, mut cell: IVec3) -> bool {
        loop {
            let o = cell.as_vec3() + 0.5 - s.c;
            let q = Vec3::new(o.x / s.r, o.y / if o.y > 0.0 { s.up } else { s.down }, o.z / s.r);
            let d = q.length_squared();
            // Bites reach no deeper than this.
            if d <= 0.55 {
                return true;
            }
            if d > 1.0 - 0.3 * self.lump(cell) - 0.12 * self.unit(cell) {
                return false;
            }
            // Only along an axis where the step brings it nearer the centre.
            let a = Vec3::select(o.abs().cmpgt(Vec3::splat(0.5)), q.abs(), Vec3::ZERO);
            if a == Vec3::ZERO {
                return true;
            }
            let axis = if a.x >= a.y && a.x >= a.z { 0 } else if a.y >= a.z { 1 } else { 2 };
            cell[axis] -= o[axis].signum() as i32;
        }
    }

    /// One layer of a conifer's tier around `c`: out to `r` where its sprays reach
    /// furthest, lobed into `lobes` sprays turned by `turn` (`depth`, 0 ..= 1, is how
    /// deeply), with a ragged rim that leaves no leaf cut off from the rest (as in
    /// `in_clump`).
    fn whorl(&mut self, c: Vec3, r: f32, lobes: f32, turn: f32, depth: f32, leaves: BlockId) {
        let shape = Whorl { c: Vec2::new(c.x, c.z), r: r.min(self.room(c)), lobes, turn, depth: depth.min(0.9) };
        let y = c.y.floor() as i32;
        let reach = shape.r + 0.45;
        let min = IVec3::new((c.x - reach).floor() as i32, y, (c.z - reach).floor() as i32);
        let max = IVec3::new((c.x + reach).floor() as i32, y, (c.z + reach).floor() as i32);
        let Some((min, max)) = self.blocks.clip(min, max) else { return };
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let cell = IVec3::new(x, y, z);
                if self.reachable(cell) && self.in_whorl(&shape, cell) {
                    self.put_leaf(cell, leaves);
                }
            }
        }
    }

    /// Whether `cell` is a leaf of a whorl (see `in_clump`).
    fn in_whorl(&self, s: &Whorl, mut cell: IVec3) -> bool {
        loop {
            let o = Vec2::new(cell.x as f32 + 0.5, cell.z as f32 + 0.5) - s.c;
            let d = o.length();
            // Every spray reaches at least this far.
            if d <= 1.1 {
                return true;
            }
            let spray = 0.5 + 0.5 * (s.lobes * o.y.atan2(o.x) + s.turn).cos();
            let reach = (s.r * (1.0 - s.depth * (1.0 - spray))).max(1.1);
            if d > reach + 0.45 - 0.45 * self.unit(cell) {
                return false;
            }
            if o.x.abs() >= o.y.abs() {
                cell.x -= o.x.signum() as i32;
            } else {
                cell.z -= o.y.signum() as i32;
            }
        }
    }

    /// Curtains of leaves (after Wilder Wild's willows) hanging from the underside of a
    /// crown of `leaves` within `reach` of `axis`, between heights `low` and `high`: from
    /// some of its lowest leaves, more towards its rim, each `drape` blocks long, stopping
    /// at anything in the way. Each block column of the column being generated hangs its
    /// own from its own blocks.
    fn curtains(&mut self, axis: Vec2, reach: f32, low: i32, high: i32, drape: (f32, f32), leaves: BlockId) {
        let r = reach.min(REACH as f32);
        let min = IVec3::new((axis.x - r).floor() as i32, low, (axis.y - r).floor() as i32);
        let max = IVec3::new((axis.x + r).floor() as i32, high, (axis.y + r).floor() as i32);
        let Some((min, max)) = self.blocks.clip(min, max) else { return };
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let d = Vec2::new(x as f32 + 0.5, z as f32 + 0.5).distance(axis) / r;
                let cell = IVec3::new(x, 0, z);
                if d > 1.0 || self.unit_of(DRAPES, cell) > 0.1 + 0.7 * smoothstep(0.25, 0.75, d) || !self.reachable(cell) {
                    continue;
                }
                let Some(bottom) = (min.y..=max.y).find(|&y| self.blocks.get(cell.with_y(y)) == leaves) else { continue };
                // Longest at the rim, where the curtains hang free.
                let length = (lerp(drape, self.unit_of(DRAPES ^ 1, cell)) * (0.5 + 0.7 * d)).round() as i32;
                for y in (bottom - length..bottom).rev() {
                    if !self.blocks.put_if_air(cell.with_y(y), leaves) {
                        break;
                    }
                    self.parts.leaves.push(cell.with_y(y));
                }
            }
        }
    }

    /// A mushroom's cap on a stem whose top is `top`: a solid, flattened dome `radius`
    /// across, and a rim hanging one block under its outer edge, every block of it joined
    /// to the rest. It grows like wood, through other trees' leaves.
    fn cap(&mut self, top: Vec3, radius: f32, cap: BlockId) {
        let r = radius.ceil() as i32;
        let (cx, y0, cz) = (top.x.floor() as i32, top.y.floor() as i32 - 1, top.z.floor() as i32);
        for z in cz - r..=cz + r {
            for x in cx - r..=cx + r {
                let d = Vec3::new(x as f32 + 0.5 - top.x, 0.0, z as f32 + 0.5 - top.z).length() / radius;
                if d > 1.0 {
                    continue;
                }
                // Thickest over the stem, thinning to one block at the edge.
                let thickness = 1 + ((1.0 - d * d) * 2.2) as i32;
                for y in y0..y0 + thickness {
                    self.wood(IVec3::new(x, y, z), cap);
                }
                if d > 0.72 {
                    self.wood(IVec3::new(x, y0 - 1, z), cap);
                }
            }
        }
    }

    /// A bush on a stem at the base: a dome of leaves `radius` across with up to three
    /// smaller ones beside it, rising from the ground at the stem and draped over the
    /// ground around it, so on a slope it sits in the ground uphill and reaches down to it
    /// downhill, never hanging over air. Its rim and top lose a block here and there.
    fn mound(&mut self, radius: f32, leaves: BlockId, stem: BlockId, rng: &mut Rng) {
        let base = self.base;
        let floor = base.y - 1;
        let centre = Vec2::new(base.x as f32 + 0.5, base.z as f32 + 0.5);
        let mut domes = [(centre, radius, radius * pick(rng, (0.85, 1.15))); 4];
        let extra = if radius > 1.2 { rng.range(1, 4) } else { rng.range(0, 2) } as usize;
        let phase = rng.unit() * TAU;
        for (i, dome) in domes.iter_mut().enumerate().skip(1).take(extra) {
            let out = heading(phase + i as f32 * GOLDEN);
            let r = radius * pick(rng, (0.5, 0.75));
            *dome = (centre + Vec2::new(out.x, out.z) * radius * pick(rng, (0.5, 0.85)), r, r * pick(rng, (0.7, 1.0)));
        }
        let domes = &domes[..=extra];
        let span = (radius * 1.9).ceil() as i32;
        let min = IVec3::new(base.x - span, floor - 3, base.z - span);
        let max = IVec3::new(base.x + span, floor + 4, base.z + span);
        if let Some((min, max)) = self.blocks.clip(min, max) {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    let p = Vec2::new(x as f32 + 0.5, z as f32 + 0.5);
                    // The height of the domes' surface here.
                    let surface = domes
                        .iter()
                        .filter_map(|&(c, r, h)| {
                            let d = p.distance(c) / (r + 0.35);
                            (d < 1.0).then(|| (floor + 1) as f32 + h * (1.0 - d * d).sqrt())
                        })
                        .fold(f32::MIN, f32::max);
                    let ground = (self.ground)(x, z);
                    // Nor over a hole the analytic ground does not know of (the lip of a
                    // sinkhole), which this block column sees in its own blocks.
                    if surface == f32::MIN || (ground - floor).abs() > 2 || !self.blocks.earth(IVec3::new(x, ground, z)) {
                        continue;
                    }
                    // Blocks of leaves over the stem's ground here, from the domes' height
                    // over it, rounded with a little randomness so the top and the rim are
                    // ragged; at least one wherever the domes stand high enough.
                    let thickness = surface - (floor + 1) as f32;
                    let u = self.unit(IVec3::new(x, 0, z));
                    if thickness < 0.2 + 0.4 * u {
                        continue;
                    }
                    // From the ground up, stopping at anything in the way (water in a
                    // hollow), so no leaf of it hangs over what it could not fill.
                    let top = floor + ((thickness + 0.35 - 0.4 * u).round() as i32).max(1);
                    for y in ground + 1..=top {
                        let cell = IVec3::new(x, y, z);
                        if !self.reachable(cell) || !self.put_leaf(cell, leaves) {
                            break;
                        }
                    }
                }
            }
        }
        // The stem, inside the leaves with leaves over it, and down into the ground.
        if !stem.is_air() {
            self.wood(base, stem);
            self.sink(base - IVec3::Y, stem);
            self.leaf(base + IVec3::Y, leaves);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
    use voxmar_engine::glam::IVec3;

    use super::*;

    fn registry() -> Arc<BlockRegistry> {
        Arc::new(BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap())
    }

    /// Grows `tree` from seed `seed` on flat grass at height 100 in an empty column, and
    /// returns the column.
    fn grown<'a>(trees: &Trees, tree: TreeId, seed: u64, vigour: f32, grass: BlockId, soft: &'a [bool], earth: &'a [bool]) -> Blocks<'a> {
        let mut blocks = Blocks::empty(IVec3::ZERO, &[], soft, earth);
        for z in 0..32 {
            for x in 0..32 {
                blocks.set(IVec3::new(x, 100, z), grass);
            }
        }
        let mut rng = Rng::new(seed, 0x7ee, 0, 0);
        trees.grow(tree, IVec3::new(16, 101, 16), vigour, &|_, _| 100, &mut rng, &mut blocks, &mut TreeParts::default());
        blocks
    }

    /// A stand mixes young trees, mature ones and the odd giant: heights spread well
    /// beyond a species' own range both ways, giants turn up more in old growth, and only
    /// old enough trees get a thick trunk.
    #[test]
    fn ages_spread_heights() {
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let trees = Trees::new(&content, &registry);
        let index = |name: &str| content.trees.iter().position(|t| t.name == format!("base:{name}")).unwrap();
        let oak = &trees.species[index("oak")];
        for vigour in [0.0, 1.0] {
            let mut rng = Rng::new(9, 9, 9, 9);
            let mut heights: Vec<i32> = (0..2000).map(|_| Size::draw(oak, vigour, &mut rng).height).collect();
            heights.sort_unstable();
            let (low, median, high) = (heights[20] as f32, heights[1000] as f32, heights[1980] as f32);
            println!("vigour {vigour}: 1% {low}, median {median}, 99% {high}");
            assert!(low <= 0.65 * median && high >= 1.3 * median, "oak heights {low}, {median}, {high} at vigour {vigour}");
        }
        let giants = |vigour: f32| {
            let mut rng = Rng::new(3, 3, 3, 3);
            (0..4000).filter(|_| Size::draw(oak, vigour, &mut rng).crown > 1.15).count()
        };
        assert!(giants(1.0) > 2 * giants(0.0) && giants(0.0) > 0, "giants: {} in old growth, {} at the edge", giants(1.0), giants(0.0));
        let spruce = &trees.species[index("giant_spruce")];
        let mut rng = Rng::new(5, 5, 5, 5);
        let sizes: Vec<Size> = (0..500).map(|_| Size::draw(spruce, 0.0, &mut rng)).collect();
        assert!(sizes.iter().any(|s| s.thick) && sizes.iter().any(|s| !s.thick));
        assert!(sizes.iter().filter(|s| s.thick).all(|s| s.crown > 0.78));
    }

    /// Grown crowns spread their leaves wide but within reach of the trunk, and fruiting
    /// trees hang their fruit where it can be seen and reached: on the underside or rim of
    /// the crown, never buried in it.
    #[test]
    fn grown_crowns_and_their_fruit() {
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let trees = Trees::new(&content, &registry);
        let soft: Vec<bool> = (0..registry.len()).map(|i| BlockId(i as u16).is_air()).collect();
        let earth: Vec<bool> = soft.iter().map(|s| !s).collect();
        let grass = registry.expect("base:grass_block");
        let apple = registry.expect("base:apple_leaves");
        let mut fruit = 0;
        for name in ["oak", "spruce", "pine", "cherry", "jungle"] {
            let index = content.trees.iter().position(|t| t.name == format!("base:{name}")).unwrap();
            let leaves = trees.species[index].leaves;
            for seed in 0..8 {
                let blocks = grown(&trees, TreeId(index as u16), 100 + seed, 0.6, grass, &soft, &earth);
                let mut cells = Vec::new();
                for y in 101..150 {
                    for z in 0..32 {
                        for x in 0..32 {
                            let b = blocks.get(IVec3::new(x, y, z));
                            if b == leaves || trees.species[index].fruit.is_some_and(|f| f.0 == b) {
                                cells.push((IVec3::new(x, y, z), b));
                            }
                        }
                    }
                }
                let spread = cells.iter().map(|(c, _)| (c.x - 16).abs().max((c.z - 16).abs())).max().unwrap_or(0);
                assert!(cells.len() > 8 && (2..=REACH).contains(&spread), "{name} {seed}: {} leaves spreading {spread}", cells.len());
                for &(c, b) in &cells {
                    if b == apple {
                        fruit += 1;
                        let open = [IVec3::NEG_Y, IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z].iter().any(|&d| blocks.get(c + d).is_air());
                        assert!(open, "{name} {seed}: fruit buried at {c}");
                    }
                }
            }
        }
        assert!(fruit > 0, "no apples on eight oaks");
    }

    /// Every block of a grown tree joins the rest through its wood and leaves: no clump of
    /// leaves floats apart from the limbs that bear it, giants included, whose crowns reach
    /// furthest.
    #[test]
    fn grown_trees_hold_together() {
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let trees = Trees::new(&content, &registry);
        let soft: Vec<bool> = (0..registry.len()).map(|i| BlockId(i as u16).is_air()).collect();
        let earth: Vec<bool> = soft.iter().map(|s| !s).collect();
        let grass = registry.expect("base:grass_block");
        for name in ["oak", "big_oak", "ancient_oak", "birch", "maple", "cherry", "acacia", "jungle", "giant_jungle", "spruce", "giant_spruce", "pine"] {
            let index = content.trees.iter().position(|t| t.name == format!("base:{name}")).unwrap();
            for seed in 0..12 {
                let blocks = grown(&trees, TreeId(index as u16), 500 + seed, 1.0, grass, &soft, &earth);
                let part = |p: IVec3| p.y > 100 && !blocks.get(p).is_air();
                let mut cells = Vec::new();
                for y in 101..160 {
                    for z in 0..32 {
                        for x in 0..32 {
                            if part(IVec3::new(x, y, z)) {
                                cells.push(IVec3::new(x, y, z));
                            }
                        }
                    }
                }
                let mut seen: std::collections::HashSet<IVec3> = std::collections::HashSet::from([IVec3::new(16, 101, 16)]);
                let mut stack = vec![IVec3::new(16, 101, 16)];
                while let Some(p) = stack.pop() {
                    for d in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
                        if part(p + d) && seen.insert(p + d) {
                            stack.push(p + d);
                        }
                    }
                }
                let loose: Vec<IVec3> = cells.iter().copied().filter(|c| !seen.contains(c)).collect();
                assert!(loose.is_empty(), "{name} {seed}: {} of {} blocks float, e.g. {}", loose.len(), cells.len(), loose[0]);
            }
        }
    }

    /// `cargo test -p voxmar_worldgen tree_preview -- --ignored --nocapture` (optionally
    /// `TREES=oak,birch`, `VIGOUR=0..1`, `SAMPLES=n`, `SCALE` pixels a block) draws every species `SAMPLES` times
    /// (four by default), young to old as their ages fall, in isometric views on a patch
    /// of grass, three species to an image, to `target/trees_N.ppm`, and prints the
    /// heights each grew to.
    #[test]
    #[ignore]
    fn tree_preview() {
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let trees = Trees::new(&content, &registry);
        let soft: Vec<bool> = (0..registry.len())
            .map(|i| {
                let def = registry.def(BlockId(i as u16));
                BlockId(i as u16).is_air() || def.name.ends_with("_leaves") || def.render == RenderKind::Plant || def.name.ends_with("water")
            })
            .collect();
        let earth: Vec<bool> = soft.iter().map(|s| !s).collect();
        let grass = registry.expect("base:grass_block");
        let env = |name: &str| std::env::var(name).ok();
        let vigour: f32 = env("VIGOUR").and_then(|v| v.parse().ok()).unwrap_or(0.6);
        let samples: i32 = env("SAMPLES").and_then(|v| v.parse().ok()).unwrap_or(4);
        let only: Option<Vec<String>> = env("TREES").map(|v| v.split(',').map(|n| format!("base:{n}")).collect());
        let species: Vec<(usize, &str)> =
            content.trees.iter().enumerate().map(|(i, t)| (i, t.name.as_str())).filter(|(_, n)| only.as_ref().is_none_or(|o| o.iter().any(|w| w == n))).collect();

        // Isometric: the viewer looks down along -(1, 1, 1), so +x, +y and +z faces show,
        // and nearer voxels (larger x + y + z) are drawn later.
        let a: f32 = env("SCALE").and_then(|v| v.parse().ok()).unwrap_or(6.0);
        const SPAN: i32 = 20;
        let project = |p: Vec3| Vec2::new((p.x - p.z) * a, (p.x + p.z) * a * 0.5 - p.y * a);
        let faces = [
            [Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)],
            [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, 0.0, 1.0)],
            [Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)],
        ];
        let inside = |q: &[Vec2; 4], p: Vec2| {
            let sides: Vec<f32> = (0..4).map(|i| (q[(i + 1) % 4] - q[i]).perp_dot(p - q[i])).collect();
            sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0)
        };
        let mut sprite: Vec<(i32, i32, usize)> = Vec::new();
        let half = (2.0 * a) as i32 + 2;
        for py in -half..half {
            for px in -half..half {
                let p = Vec2::new(px as f32 + 0.5, py as f32 + 0.5);
                if let Some(face) = (0..3).find(|&f| inside(&faces[f].map(project), p)) {
                    sprite.push((px, py, face));
                }
            }
        }
        let width = (2.0 * SPAN as f32 * a) as i32 + 8;
        let floor = (SPAN as f32 * a * 0.5) as i32 + 8;
        for (page, chunk) in species.chunks(3).enumerate() {
            // Each species' samples, as voxels sorted far to near, and the tallest.
            let rows: Vec<(Vec<(IVec3, BlockId)>, i32)> = chunk
                .iter()
                .flat_map(|&(index, _)| (0..samples).map(move |sample| (index, sample)))
                .map(|(index, sample)| {
                    let blocks = grown(&trees, TreeId(index as u16), 1000 + sample as u64 * 7919, vigour, grass, &soft, &earth);
                    let mut cells = Vec::new();
                    for y in 98..170 {
                        for z in 16 - SPAN / 2..16 + SPAN / 2 {
                            for x in 16 - SPAN / 2..16 + SPAN / 2 {
                                let b = blocks.get(IVec3::new(x, y, z));
                                if !b.is_air() {
                                    cells.push((IVec3::new(x, y, z), b));
                                }
                            }
                        }
                    }
                    let tallest = cells.iter().map(|c| c.0.y - 100).max().unwrap_or(0);
                    cells.sort_by_key(|c| c.0.x + c.0.y + c.0.z);
                    (cells, tallest)
                })
                .collect();
            let heights: Vec<i32> = chunk.iter().enumerate().map(|(r, _)| rows[r * samples as usize..(r + 1) * samples as usize].iter().map(|t| t.1).max().unwrap()).collect();
            let tops: Vec<i32> = heights.iter().scan(0, |y, &h| {
                let top = *y;
                *y += floor + (h + 2) * a as i32;
                Some(top)
            }).collect();
            let (w, h) = (width * samples, tops.last().unwrap() + floor + (heights.last().unwrap() + 2) * a as i32);
            let mut image = vec![[200u8, 214, 230]; (w * h) as usize];
            for (row, &(_, name)) in chunk.iter().enumerate() {
                let samples_here = &rows[row * samples as usize..(row + 1) * samples as usize];
                println!("{name}: heights {:?}", samples_here.iter().map(|t| t.1).collect::<Vec<_>>());
                for (sample, (cells, _)) in samples_here.iter().enumerate() {
                    let origin = Vec2::new((sample as i32 * width + width / 2) as f32, (tops[row] + (heights[row] + 2) * a as i32) as f32);
                    for &(at, b) in cells {
                        let def = registry.def(b);
                        let leafy = def.name.ends_with("leaves");
                        let speckle = 0.85 + 0.3 * ((crate::rng::hash3(7, at.x, at.y, at.z) & 0xff) as f32 / 255.0);
                        let base = Vec3::new(def.color[0] as f32, def.color[1] as f32, def.color[2] as f32) * if leafy { speckle } else { 1.0 };
                        let corner = project(at.as_vec3() - Vec3::new(16.0, 101.0, 16.0)) + origin;
                        for &(px, py, face) in &sprite {
                            let (x, y) = (corner.x as i32 + px, corner.y as i32 + py);
                            if x < 0 || y < 0 || x >= w || y >= h {
                                continue;
                            }
                            let shade = [1.0, 0.78, 0.6][face];
                            let c = (base * shade).min(Vec3::splat(255.0));
                            image[(y * w + x) as usize] = [c.x as u8, c.y as u8, c.z as u8];
                        }
                    }
                }
            }
            let mut out = format!("P6 {w} {h} 255\n").into_bytes();
            out.extend(image.iter().flatten());
            let path = format!("{}/../../target/trees_{page}.ppm", env!("CARGO_MANIFEST_DIR"));
            std::fs::write(&path, out).unwrap();
            println!("wrote {path}: {}", chunk.iter().map(|c| c.1).collect::<Vec<_>>().join(", "));
        }
    }
}
