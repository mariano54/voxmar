//! Procedural trees, in the spirit of TanTanDev's shrubbery: a species is a handful of
//! parameters (trunk height, thickness and lean, roots, and a crown shape) defined in
//! `trees.ron`, and each tree
//! lays out its trunk, branches and leaf shapes in continuous space from its own random
//! stream, then voxelizes them. Randomness tied to a voxel comes from hashing its
//! position, never from the stream, so every column a tree reaches into builds the same
//! tree and keeps only its own part.

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::{IVec3, Vec3};

use crate::content::{Crown, Roots, WorldContent};
use crate::rng::{Rng, hash3};
use crate::terrain::Blocks;

/// Furthest any part of a tree reaches from its base, horizontally.
pub const REACH: i32 = 8;

/// A kind of tree, by its place in the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TreeId(pub u16);

struct Species {
    log: BlockId,
    /// Bark on every side, for trunks lying on the ground: the wood of the log's family,
    /// or the log itself.
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
                    roots_block: def.root_block.as_deref().map_or(log, |name| registry.expect(name)),
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
    /// forest's stand) favours the tall end of the species' heights: old-growth trees are
    /// taller than those at a scrubby edge.
    pub fn grow(&self, tree: TreeId, base: IVec3, vigour: f32, rng: &mut Rng, blocks: &mut Blocks<'_>, parts: &mut TreeParts) {
        let s = &self.species[tree.0 as usize];
        parts.logs.clear();
        parts.leaves.clear();
        let mut g = Grower { blocks, seed: rng.next_u64(), base, parts };
        let leaves = match s.alternate {
            Some((alternate, share)) if rng.chance(share) => alternate,
            _ => s.leaves,
        };
        let size = (0.15 + 0.6 * vigour + 0.5 * rng.unit()).min(0.999);
        let height = s.height.0 + ((s.height.1 - s.height.0 + 1) as f32 * size) as i32;
        let pick = |rng: &mut Rng, (lo, hi): (f32, f32)| lo + (hi - lo) * rng.unit();
        let count = |rng: &mut Rng, (lo, hi): (i32, i32)| rng.range(lo, hi + 1);

        // The trunk drifts sideways along a gentle curve; `trunk(t)` is its centre line.
        let thick = if s.thick { 0.5 } else { 0.0 };
        let root = Vec3::new(base.x as f32 + 0.5 + thick, base.y as f32, base.z as f32 + 0.5 + thick);
        let angle = rng.unit() * std::f32::consts::TAU;
        let lean = Vec3::new(angle.cos(), 0.0, angle.sin()) * s.lean * rng.unit();
        let lift = if s.roots == Roots::Stilts { 2.0 } else { 0.0 };
        let trunk = |t: f32| root + Vec3::Y * (lift + height as f32 * t) + lean * t * t;
        let top = trunk(1.0);
        if !matches!(s.crown, Crown::Bush { .. }) {
            // Trees on stilts stand on their roots; others reach a little into the ground.
            g.trunk(trunk, height, s.thick, s.log, s.roots != Roots::Stilts);
        }

        match s.roots {
            Roots::None => {}
            Roots::Buttress => {
                for i in 0..4 {
                    let a = (i as f32 + 0.3 * rng.unit()) * std::f32::consts::FRAC_PI_2;
                    let out = Vec3::new(a.cos(), 0.0, a.sin());
                    let from = root + out * 0.8 + Vec3::Y * pick(rng, (2.5, 4.5));
                    g.line(from, root + out * pick(rng, (2.5, 3.5)) - Vec3::Y, s.roots_block);
                }
            }
            Roots::Stilts => {
                for _ in 0..count(rng, (4, 6)) {
                    let a = rng.unit() * std::f32::consts::TAU;
                    let out = Vec3::new(a.cos(), 0.0, a.sin());
                    let from = root + Vec3::Y * pick(rng, (2.5, 4.0));
                    let foot = root + out * pick(rng, (1.8, 3.0)) - Vec3::Y * 2.0;
                    let knee = (from + foot) * 0.5 + out * 0.8 + Vec3::Y * 0.8;
                    g.line(from, knee, s.roots_block);
                    g.line(knee, foot, s.roots_block);
                }
            }
        }

        match s.crown {
            Crown::Round { branches, reach, radius, squash } => {
                let n = count(rng, branches);
                let phase = rng.unit() * std::f32::consts::TAU;
                for i in 0..n {
                    let a = phase + (i as f32 + 0.4 * rng.unit()) * std::f32::consts::TAU / n as f32;
                    let from = trunk(pick(rng, (0.55, 0.9)));
                    let dir = Vec3::new(a.cos(), pick(rng, (0.5, 1.1)), a.sin()).normalize();
                    let end = from + dir * pick(rng, reach);
                    g.line(from, end, s.log);
                    let r = pick(rng, radius);
                    g.blob(end + Vec3::Y * 0.5, Vec3::new(r, r * squash, r), leaves);
                }
                let r = pick(rng, radius) + 0.3;
                g.blob(top + Vec3::Y * 0.5, Vec3::new(r, r * squash, r), leaves);
            }
            Crown::Conical { radius, start } => {
                // Layers around a thick trunk are measured from its surface rather than its
                // centre line, so they still wrap it.
                let girth = if s.thick { 1.0 } else { 0.0 };
                let base_radius = pick(rng, radius);
                let first = (height as f32 * start) as i32;
                let span = (height + 2 - first) as f32;
                for y in first..=height + 1 {
                    let t = (y - first) as f32 / span;
                    // Whorls: every other layer steps in, so the cone has tiers.
                    let tier = if (height - y) % 2 == 0 { 1.0 } else { 0.65 };
                    let r = (base_radius * (1.0 - t) * tier).max(if y <= height { 0.8 } else { 0.0 }) + girth;
                    g.disc(trunk(y as f32 / height as f32), r, leaves);
                }
                g.blob(top + Vec3::Y * 1.5, Vec3::new(0.5 + girth, 1.2, 0.5 + girth), leaves);
            }
            Crown::Spire { radius, start } => {
                // Radius along the crown: 0 at its foot, widest a third of the way up, 0 at
                // its tip, which stands a third of the trunk above the trunk's top.
                let r = pick(rng, radius);
                let first = (height as f32 * start) as i32;
                let last = height + height / 3;
                let span = (last + 1 - first) as f32;
                for y in first..=last {
                    let x = (y - first) as f32 / span + 0.5 / span;
                    let width = r * 6.25 * x * (1.0 - x) * (1.0 - x);
                    g.disc(trunk((y as f32 / height as f32).min(1.0)), width.max(0.6), leaves);
                }
            }
            Crown::Cap { radius } => {
                // On the stem's last block, wherever its lean took it.
                let stem = trunk((height - 1) as f32 / height as f32).floor();
                g.cap(stem + Vec3::new(0.5, 1.0, 0.5), pick(rng, radius), leaves);
            }
            Crown::Umbrella { branches, reach, radius } => {
                let n = count(rng, branches);
                let phase = rng.unit() * std::f32::consts::TAU;
                for i in 0..n {
                    let a = phase + (i as f32 + 0.5 * rng.unit()) * std::f32::consts::TAU / n as f32;
                    let from = trunk(pick(rng, (0.45, 0.8)));
                    let out = Vec3::new(a.cos(), 0.0, a.sin());
                    let end = from + out * pick(rng, reach) + Vec3::Y * pick(rng, (1.5, 3.0));
                    g.line(from, end, s.log);
                    let r = pick(rng, radius);
                    g.blob(end + Vec3::Y * 0.6, Vec3::new(r, 0.9, r), leaves);
                    g.blob(end + Vec3::Y * 1.4, Vec3::new(r - 1.0, 0.6, r - 1.0), leaves);
                }
                let r = pick(rng, radius);
                g.blob(top + Vec3::Y * 0.6, Vec3::new(r, 0.9, r), leaves);
            }
            Crown::Weeping { radius, drape } => {
                let r = pick(rng, radius);
                g.blob(top, Vec3::new(r, r * 0.7, r), leaves);
                for _ in 0..count(rng, drape) {
                    let a = rng.unit() * std::f32::consts::TAU;
                    let rim = top + Vec3::new(a.cos(), 0.0, a.sin()) * (r - 0.3) - Vec3::Y * (r * 0.4);
                    let length = pick(rng, (2.0, 5.0));
                    g.line_of(rim, rim - Vec3::Y * length, leaves);
                }
            }
            Crown::Palm { fronds, length } => {
                let n = count(rng, fronds);
                let phase = rng.unit() * std::f32::consts::TAU;
                for i in 0..n {
                    let a = phase + (i as f32 + 0.3 * rng.unit()) * std::f32::consts::TAU / n as f32;
                    let out = Vec3::new(a.cos(), 0.0, a.sin());
                    let l = pick(rng, length);
                    // Up and out, then drooping towards the tip.
                    let mid = top + out * l * 0.5 + Vec3::Y * 0.8;
                    g.line_of(top + Vec3::Y * 0.5, mid, leaves);
                    g.line_of(mid, top + out * l - Vec3::Y * pick(rng, (0.5, 1.8)), leaves);
                }
                g.blob(top + Vec3::Y * 0.5, Vec3::new(1.0, 0.6, 1.0), leaves);
            }
            Crown::Bush { radius } => {
                let r = pick(rng, radius);
                g.wood(base, s.log);
                g.blob(root + Vec3::Y * 0.6, Vec3::new(r, r * 0.7, r), leaves);
            }
            Crown::Dead { branches, reach } => {
                for _ in 0..count(rng, branches) {
                    let a = rng.unit() * std::f32::consts::TAU;
                    let from = trunk(pick(rng, (0.5, 0.9)));
                    let dir = Vec3::new(a.cos(), pick(rng, (0.4, 1.2)), a.sin()).normalize();
                    g.line(from, from + dir * pick(rng, reach), s.log);
                }
            }
            Crown::Bare => {}
        }
    }
}

/// A bush: a stub of `log` (none if air) on the ground at `base` under a mound of
/// `leaves` `radius` blocks across, ragged at its edge. Its parts go into `parts`.
pub fn bush(base: IVec3, radius: f32, leaves: BlockId, log: BlockId, rng: &mut Rng, blocks: &mut Blocks<'_>, parts: &mut TreeParts) {
    parts.logs.clear();
    parts.leaves.clear();
    if blocks.contains(base) && blocks.get(base - IVec3::Y).is_air() {
        return;
    }
    let mut g = Grower { blocks, seed: rng.next_u64(), base, parts };
    if !log.is_air() {
        g.wood(base, log);
    }
    let centre = Vec3::new(base.x as f32 + 0.5, base.y as f32 + 0.25, base.z as f32 + 0.5);
    g.blob(centre, Vec3::new(radius, radius * 0.75 + 0.3, radius), leaves);
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
    /// Seeds per-voxel randomness (ragged leaf edges).
    seed: u64,
    base: IVec3,
    parts: &'a mut TreeParts,
}

impl Grower<'_, '_> {
    fn unit(&self, cell: IVec3) -> f32 {
        (hash3(self.seed, cell.x, cell.y, cell.z) & 0xffff) as f32 / 65536.0
    }

    fn reachable(&self, cell: IVec3) -> bool {
        (cell.x - self.base.x).abs() <= REACH && (cell.z - self.base.z).abs() <= REACH
    }

    fn wood(&mut self, cell: IVec3, log: BlockId) {
        if self.reachable(cell) && self.blocks.put_log(cell, log) {
            self.parts.logs.push(cell);
        }
    }

    fn leaf(&mut self, cell: IVec3, leaves: BlockId) {
        if self.reachable(cell) {
            self.put_leaf(cell, leaves);
        }
    }

    /// Puts a leaf at a cell already known to be within reach.
    fn put_leaf(&mut self, cell: IVec3, leaves: BlockId) {
        if !leaves.is_air() && self.blocks.put_if_air(cell, leaves) {
            self.parts.leaves.push(cell);
        }
    }

    /// Logs along the trunk's centre line `centre(0..1)`. A sunk trunk continues two
    /// blocks below its base, filling only air, so trunks on slopes (and every corner of
    /// a 2×2 trunk) reach the ground.
    fn trunk(&mut self, centre: impl Fn(f32) -> Vec3, height: i32, thick: bool, log: BlockId, sunk: bool) {
        let offsets: &[(i32, i32)] = if thick { &[(0, 0), (-1, 0), (0, -1), (-1, -1)] } else { &[(0, 0)] };
        let mut last: Option<IVec3> = None;
        for k in if sunk { -2 } else { 0 }..height {
            let p = centre(k.max(0) as f32 / height as f32);
            let cell = IVec3::new(p.x.floor() as i32, p.y.floor() as i32 + k.min(0), p.z.floor() as i32);
            for step in connect(last.unwrap_or(cell), cell) {
                for &(dx, dz) in offsets {
                    let cell = step + IVec3::new(dx, 0, dz);
                    if k >= 0 {
                        self.wood(cell, log);
                    } else if self.reachable(cell) && self.blocks.put_if_air(cell, log) {
                        self.parts.logs.push(cell);
                    }
                }
            }
            last = Some(cell);
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

    /// A line of leaves: fronds, hanging curtains.
    fn line_of(&mut self, from: Vec3, to: Vec3, leaves: BlockId) {
        for cell in Self::line_cells(from, to) {
            self.leaf(cell, leaves);
        }
    }

    /// An ellipsoid of leaves whose outer shell is only partly filled.
    fn blob(&mut self, center: Vec3, radii: Vec3, leaves: BlockId) {
        let reach = IVec3::new(REACH, 0, REACH);
        let min = (center - radii).floor().as_ivec3().max(self.base - reach.with_y(1000));
        let max = (center + radii).floor().as_ivec3().min(self.base + reach.with_y(1000));
        let Some((min, max)) = self.blocks.clip(min, max) else { return };
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    let cell = IVec3::new(x, y, z);
                    let d = (cell.as_vec3() + 0.5 - center) / radii;
                    if d.length_squared() <= 1.0 - 0.35 * self.unit(cell) {
                        self.put_leaf(cell, leaves);
                    }
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

    /// One layer of leaves around `center`, with a ragged rim.
    fn disc(&mut self, center: Vec3, radius: f32, leaves: BlockId) {
        let r = radius.ceil() as i32;
        let (cx, y, cz) = (center.x.floor() as i32, center.y.floor() as i32, center.z.floor() as i32);
        for z in cz - r..=cz + r {
            for x in cx - r..=cx + r {
                let cell = IVec3::new(x, y, z);
                let offset = Vec3::new(x as f32 + 0.5 - center.x, 0.0, z as f32 + 0.5 - center.z);
                if offset.length() <= radius + 0.2 - 0.45 * self.unit(cell) {
                    self.leaf(cell, leaves);
                }
            }
        }
    }
}
