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
//! - **Crowns of clumps.** Broadleaf crowns are clumps of leaves at the ends of limbs set
//!   around the trunk at the golden angle, flat beneath and lumpy at the edge, with gaps
//!   between them where the limbs show. Conifers are tiers of lobed whorls with drooping
//!   tips under a spike; pines hold pads of needles on upswept limbs; willows hang
//!   curtains from the underside of their clumps; acacias spread flat pads on forked limbs.
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
        let young = 0.1 + 0.25 * (1.0 - vigour);
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
        let mut g = Grower { blocks, seed: rng.next_u64(), base, parts, ground };
        let leaves = match s.alternate {
            Some((alternate, share)) if rng.chance(share) => alternate,
            _ => s.leaves,
        };
        let size = Size::draw(s, vigour, rng);
        let (height, k) = (size.height, size.crown);
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
        let lean = heading(angle) * s.lean * rng.unit();
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
            Roots::None => g.flare(&feet, if size.thick { size.flare.max(0.4) } else { size.flare }, s.wood),
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
            Crown::Round { branches, reach, radius, squash, start } => {
                // Limbs climb the upper trunk at the golden angle, the lower ones longer,
                // each ending in a clump; long limbs carry a smaller clump midway.
                let n = ((count(rng, branches) as f32 * (0.55 + 0.45 * k)).round() as i32).max(1);
                for i in 0..n {
                    let f = (i as f32 + 0.25 + 0.5 * rng.unit()) / n as f32;
                    let from = on_trunk(lerp((start, 0.92), f));
                    let out = heading(phase + i as f32 * GOLDEN + 0.5 * (rng.unit() - 0.5));
                    let r = pick(rng, radius) * k;
                    let length = (pick(rng, reach) * k * (1.15 - 0.35 * f)).min((g.room(from) - 0.6 * r).max(0.5));
                    let rise = pick(rng, (0.3, 0.8));
                    let elbow = from + out * length * 0.5 + Vec3::Y * (length * rise * 0.4);
                    let end = from + out * length + Vec3::Y * (length * rise + 0.5);
                    g.line(from, elbow, s.wood);
                    g.line(elbow, end, s.wood);
                    g.clump(end + Vec3::Y * 0.3, r, r * squash, r * squash * 0.6, leaves);
                    if length > 2.0 && rng.chance(0.55) {
                        let side = Vec3::new(-out.z, 0.0, out.x) * if rng.chance(0.5) { 0.8 } else { -0.8 };
                        let r = r * pick(rng, (0.5, 0.7));
                        g.clump(elbow + side + Vec3::Y * 0.9, r, r * squash, r * squash * 0.55, leaves);
                    }
                }
                // The core: a clump round the upper trunk ties the limbs' clumps into one
                // crown, and a crowning one tops it.
                let r = pick(rng, radius) * k;
                g.clump(trunk(lerp((start, 1.0), 0.6)), r * 0.9, r * squash * 0.9, r * squash * 0.6, leaves);
                let r = pick(rng, radius) * k * 1.05;
                g.clump(top + Vec3::Y * (0.4 + 0.3 * r * squash), r, r * squash, r * squash * 0.6, leaves);
            }
            Crown::Conical { radius, start, ragged } => {
                // Tiers from the top down: each a wide skirt of lobed sprays with a narrower
                // layer over it and tips drooping at its rim, limbs inside the lower ones.
                // Layers around a thick trunk are measured from its surface, so they wrap it.
                let wrap = if size.thick { 1.0 } else { 0.0 };
                let widest = pick(rng, radius) * k;
                let first = ((height as f32 * start * pick(rng, (0.8, 1.2))) as i32).max(1);
                let spike = height + 1 + i32::from(k > 1.1);
                let span = (spike - first).max(1) as f32;
                let at = |y: i32| {
                    let c = trunk((y as f32 / height as f32).min(1.0));
                    Vec3::new(c.x, root.y + lift + y as f32 + 0.5, c.z)
                };
                let mut y = spike - 1;
                while y > first {
                    let t = (y - first) as f32 / span;
                    let wide = widest * (1.0 - t).powf(0.9) + 0.7;
                    let lobes = rng.range(3, 6) as f32;
                    let turn = rng.unit() * TAU;
                    let depth = ragged * pick(rng, (0.6, 1.3));
                    g.whorl(at(y - 1), wide + wrap, lobes, turn, depth, leaves);
                    g.whorl(at(y), wide * 0.55 + wrap, lobes, turn + 0.6, depth * 0.5, leaves);
                    if wide > 1.4 {
                        g.droop(at(y - 2), wide + wrap, lobes, turn, leaves);
                    }
                    // Limbs inside the lower tiers, seen from beneath but kept short of the
                    // rim, where they would speckle the foliage.
                    if wide > 1.8 && t < 0.5 {
                        for j in 0..lobes as i32 {
                            let out = heading((j as f32 * TAU - turn) / lobes);
                            g.line(at(y - 1), at(y - 1) + out * (wide * 0.45 + wrap), s.wood);
                        }
                    }
                    // Lower down, now and then a wider gap between tiers.
                    y -= if t < 0.5 && rng.chance(ragged) { 3 } else { 2 };
                }
                // The leader: a spike of leaves over the top tier.
                let tip = at(spike);
                for dy in 0..=i32::from(k > 0.9) {
                    g.leaf(tip.floor().as_ivec3() + IVec3::Y * dy, leaves);
                }
            }
            Crown::Pine { radius, start } => {
                // A rounded, clumpy cone of needle pads on short upswept limbs over the upper
                // trunk, narrowing upwards to a flattish top; stubs of dead limbs below,
                // where the lower crown died off.
                let widest = pick(rng, radius) * k;
                let first = start * pick(rng, (0.9, 1.1));
                let tiers = (((1.0 - first) * height as f32 / 1.4).round() as i32).max(2);
                for i in 0..tiers {
                    let f = (i as f32 + 0.5 * rng.unit()) / tiers as f32;
                    let from = on_trunk(lerp((first, 0.95), f));
                    let reach = widest * (1.0 - 0.7 * f);
                    for j in 0..rng.range(1, 4) {
                        let out = heading(phase + (3 * i + j) as f32 * GOLDEN + 0.4 * (rng.unit() - 0.5));
                        let r = (reach * 0.6).clamp(1.2, 2.6) * pick(rng, (0.85, 1.15));
                        let length = (reach * pick(rng, (0.45, 0.8))).min((g.room(from) - 0.6 * r).max(0.5));
                        let end = from + out * length + Vec3::Y * pick(rng, (0.5, 1.2));
                        g.line(from, end, s.wood);
                        g.clump(end + Vec3::Y * 0.2, r, 1.1, 0.6, leaves);
                    }
                }
                // The core: needles all along the crown's trunk, binding the pads into one
                // dark mass, and a flattish dome on top.
                let low = trunk(lerp((first, 1.0), 0.25));
                let r = (widest * 0.3).clamp(1.0, 1.6);
                let span = top.y - low.y;
                g.clump(low.lerp(top, 0.5), r, span * 0.5, span * 0.5, leaves);
                let r = (widest * 0.45).clamp(1.3, 2.2);
                g.clump(top + Vec3::Y * 0.4, r, 1.2, 0.7, leaves);
                for _ in 0..rng.range(0, 3) {
                    let from = on_trunk(pick(rng, (0.3, first)));
                    let out = heading(rng.unit() * TAU);
                    g.line(from, from + out * pick(rng, (0.8, 1.6)) + Vec3::Y * 0.4, s.wood);
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
            Crown::Umbrella { branches, reach, radius, squash, fork } => {
                // Limbs fork off the upper trunk, rise and spread, each under a flat pad
                // with a smaller one on top; the leader carries a pad of its own.
                let n = count(rng, branches);
                for i in 0..n {
                    let out = heading(phase + (i as f32 + 0.5 * rng.unit()) * TAU / n as f32);
                    let from = on_trunk(pick(rng, (fork, (fork + 0.35).min(0.97))));
                    let r = pick(rng, radius) * k;
                    let length = (pick(rng, reach) * k).min((g.room(from) - 0.6 * r).max(0.5));
                    let up = pick(rng, (1.5, 3.0)) * k.sqrt();
                    let elbow = from + out * length * 0.45 + Vec3::Y * up * 0.75;
                    let end = from + out * length + Vec3::Y * up;
                    g.line(from, elbow, s.wood);
                    g.line(elbow, end, s.wood);
                    let thick = (r * squash).max(0.9);
                    g.clump(end + Vec3::Y * 0.5, r, thick, 0.6, leaves);
                    g.clump(end + Vec3::Y * (0.5 + thick), r * 0.55, thick * 0.7, 0.5, leaves);
                }
                let r = pick(rng, radius) * k * 0.8;
                g.clump(top + Vec3::Y * 0.6, r, (r * squash).max(0.9), 0.6, leaves);
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
            Crown::Dead { branches, reach } => {
                // A snag: a few limbs stepping up and out at angles of their own, never
                // across the trunk from each other at one height.
                let n = count(rng, branches);
                for i in 0..n {
                    let f = (i as f32 + rng.unit()) / n as f32;
                    let from = on_trunk(lerp((0.35, 0.9), f)).floor().as_ivec3();
                    let out = heading(phase + i as f32 * GOLDEN + 0.4 * (rng.unit() - 0.5));
                    let length = pick(rng, reach) * k.sqrt() * (1.1 - 0.4 * f);
                    let rise = (length * pick(rng, (0.7, 1.4))).round().max(1.0) as i32;
                    let to = from + IVec3::new((out.x * length).round() as i32, rise, (out.z * length).round() as i32);
                    g.walk(from, to, s.wood);
                }
            }
            Crown::Bare => {}
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
    let mut g = Grower { blocks, seed: rng.next_u64(), base, parts, ground };
    g.mound(radius, leaves, stem, rng);
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

    /// The tips of a whorl's sprays drooping a block below it, at the outer part of the
    /// sprays, each under a leaf of the whorl (in its own block column, so every column
    /// agrees).
    fn droop(&mut self, c: Vec3, r: f32, lobes: f32, turn: f32, leaves: BlockId) {
        let r = r.min(self.room(c));
        let y = c.y.floor() as i32;
        let min = IVec3::new((c.x - r).floor() as i32, y, (c.z - r).floor() as i32);
        let max = IVec3::new((c.x + r).floor() as i32, y, (c.z + r).floor() as i32);
        let Some((min, max)) = self.blocks.clip(min, max) else { return };
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let cell = IVec3::new(x, y, z);
                let o = Vec2::new(x as f32 + 0.5 - c.x, z as f32 + 0.5 - c.z);
                let d = o.length() / r;
                let spray = 0.5 + 0.5 * (lobes * o.y.atan2(o.x) + turn).cos();
                if (0.6..=0.95).contains(&d) && spray > 0.55 && self.unit(cell) < 0.55 && self.reachable(cell) && self.blocks.get(cell + IVec3::Y) == leaves {
                    self.put_leaf(cell, leaves);
                }
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

    /// `cargo test -p voxmar_worldgen tree_preview -- --ignored --nocapture` (optionally
    /// `TREES=oak,birch`, `VIGOUR=0..1`, `SAMPLES=n`) draws every species `SAMPLES` times
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
        const A: f32 = 6.0;
        const SPAN: i32 = 20;
        let project = |p: Vec3| Vec2::new((p.x - p.z) * A, (p.x + p.z) * A * 0.5 - p.y * A);
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
        for py in -16..16 {
            for px in -16..16 {
                let p = Vec2::new(px as f32 + 0.5, py as f32 + 0.5);
                if let Some(face) = (0..3).find(|&f| inside(&faces[f].map(project), p)) {
                    sprite.push((px, py, face));
                }
            }
        }
        let width = (2.0 * SPAN as f32 * A) as i32 + 8;
        let floor = (SPAN as f32 * A * 0.5) as i32 + 8;
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
                *y += floor + (h + 2) * A as i32;
                Some(top)
            }).collect();
            let (w, h) = (width * samples, tops.last().unwrap() + floor + (heights.last().unwrap() + 2) * A as i32);
            let mut image = vec![[200u8, 214, 230]; (w * h) as usize];
            for (row, &(_, name)) in chunk.iter().enumerate() {
                let samples_here = &rows[row * samples as usize..(row + 1) * samples as usize];
                println!("{name}: heights {:?}", samples_here.iter().map(|t| t.1).collect::<Vec<_>>());
                for (sample, (cells, _)) in samples_here.iter().enumerate() {
                    let origin = Vec2::new((sample as i32 * width + width / 2) as f32, (tops[row] + (heights[row] + 2) * A as i32) as f32);
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
