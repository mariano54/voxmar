//! Procedural trees, in the spirit of TanTanDev's shrubbery: a species is a handful of
//! parameters (trunk height, thickness and lean, roots, and a crown shape), and each tree
//! lays out its trunk, branches and leaf shapes in continuous space from its own random
//! stream, then voxelizes them. Randomness tied to a voxel comes from hashing its
//! position, never from the stream, so every column a tree reaches into builds the same
//! tree and keeps only its own part.

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::{IVec3, Vec3};

use crate::rng::{Rng, hash3};
use crate::terrain::Blocks;

/// Furthest any part of a tree reaches from its base, horizontally.
pub const REACH: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tree {
    Oak,
    BigOak,
    Birch,
    Spruce,
    GiantSpruce,
    Pine,
    DarkOak,
    Cherry,
    Jungle,
    GiantJungle,
    JungleBush,
    Acacia,
    Willow,
    Mangrove,
    Palm,
    Shrub,
    Dead,
    Cactus,
}

impl Tree {
    pub const ALL: [Tree; 18] = [
        Tree::Oak,
        Tree::BigOak,
        Tree::Birch,
        Tree::Spruce,
        Tree::GiantSpruce,
        Tree::Pine,
        Tree::DarkOak,
        Tree::Cherry,
        Tree::Jungle,
        Tree::GiantJungle,
        Tree::JungleBush,
        Tree::Acacia,
        Tree::Willow,
        Tree::Mangrove,
        Tree::Palm,
        Tree::Shrub,
        Tree::Dead,
        Tree::Cactus,
    ];
}

/// The shape of the leafy part of a tree.
#[derive(Clone, Copy, Debug)]
enum Crown {
    /// Leaf blobs at the ends of branches spreading from the upper trunk, and one on top.
    /// `squash` flattens the blobs.
    Round { branches: (i32, i32), reach: (f32, f32), radius: (f32, f32), squash: f32 },
    /// Whorls of leaves narrowing to a tip, from `start` (a fraction of the trunk) up.
    Conical { radius: (f32, f32), start: f32 },
    /// Flat canopies on a few long branches that fork off the trunk.
    Umbrella { branches: (i32, i32), reach: (f32, f32), radius: (f32, f32) },
    /// A round crown with curtains of leaves hanging from its rim.
    Weeping { radius: (f32, f32), drape: (i32, i32) },
    /// Long drooping fronds at the top of a bare trunk.
    Palm { fronds: (i32, i32), length: (f32, f32) },
    /// A low mound of leaves on a stub of wood.
    Bush { radius: (f32, f32) },
    /// Bare branches.
    Dead { branches: (i32, i32), reach: (f32, f32) },
    /// No crown at all (cacti).
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Roots {
    None,
    /// Fins of wood flaring out from the base of a giant trunk.
    Buttress,
    /// Arching roots that hold the trunk above water.
    Stilts,
}

struct Species {
    log: BlockId,
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

pub struct Trees {
    species: Vec<Species>,
}

impl Trees {
    pub fn new(registry: &BlockRegistry) -> Self {
        let id = |name: &str| registry.expect(&format!("base:{name}"));
        let species = |wood: &str, leaves: &str, height, thick, lean, roots, crown| Species {
            log: id(&format!("{wood}_log")),
            leaves: id(&format!("{leaves}_leaves")),
            alternate: None,
            roots_block: id(&format!("{wood}_log")),
            height,
            thick,
            lean,
            roots,
            crown,
        };
        let round = |branches, reach, radius, squash| Crown::Round { branches, reach, radius, squash };
        let cone = |radius, start| Crown::Conical { radius, start };
        let umbrella = |branches, reach, radius| Crown::Umbrella { branches, reach, radius };
        // Lean + branch reach + crown radius stays within `REACH`.
        let species = Tree::ALL
            .map(|tree| match tree {
                Tree::Oak => species("oak", "oak", (4, 7), false, 0.6, Roots::None, round((2, 4), (1.5, 2.5), (2.0, 2.8), 0.8)),
                Tree::BigOak => species("oak", "oak", (8, 12), false, 0.8, Roots::None, round((4, 6), (2.5, 4.0), (2.2, 3.0), 0.75)),
                Tree::Birch => species("birch", "birch", (5, 9), false, 0.4, Roots::None, round((1, 3), (0.5, 1.5), (1.6, 2.2), 1.3)),
                Tree::Spruce => species("spruce", "spruce", (7, 12), false, 0.2, Roots::None, cone((2.2, 3.2), 0.25)),
                Tree::GiantSpruce => species("spruce", "spruce", (16, 24), true, 0.3, Roots::None, cone((3.5, 5.0), 0.4)),
                Tree::Pine => species("spruce", "spruce", (9, 14), false, 0.5, Roots::None, cone((2.2, 3.0), 0.5)),
                Tree::DarkOak => species("dark_oak", "dark_oak", (6, 9), true, 0.8, Roots::None, round((3, 5), (2.0, 3.5), (2.8, 3.6), 0.6)),
                Tree::Cherry => species("cherry", "cherry", (4, 7), false, 0.8, Roots::None, umbrella((2, 4), (2.5, 4.0), (2.5, 3.2))),
                Tree::Jungle => species("jungle", "jungle", (8, 13), false, 0.5, Roots::None, round((2, 4), (2.0, 3.0), (2.2, 3.0), 0.7)),
                Tree::GiantJungle => {
                    species("jungle", "jungle", (18, 26), true, 0.6, Roots::Buttress, round((3, 5), (2.5, 4.0), (2.8, 3.4), 0.65))
                }
                Tree::JungleBush => species("jungle", "oak", (1, 2), false, 0.0, Roots::None, Crown::Bush { radius: (1.6, 2.6) }),
                Tree::Acacia => species("acacia", "acacia", (4, 7), false, 1.0, Roots::None, umbrella((1, 3), (2.5, 3.8), (2.5, 3.2))),
                Tree::Willow => {
                    species("oak", "oak", (5, 8), false, 0.8, Roots::None, Crown::Weeping { radius: (2.8, 3.8), drape: (10, 18) })
                }
                Tree::Mangrove => Species {
                    roots_block: id("mangrove_roots"),
                    ..species("mangrove", "mangrove", (5, 8), false, 0.6, Roots::Stilts, round((2, 3), (1.5, 2.5), (2.2, 2.8), 0.7))
                },
                Tree::Palm => species("jungle", "jungle", (6, 10), false, 3.0, Roots::None, Crown::Palm { fronds: (6, 8), length: (3.5, 5.0) }),
                Tree::Shrub => Species {
                    alternate: Some((id("flowering_azalea_leaves"), 0.35)),
                    ..species("oak", "azalea", (1, 2), false, 0.0, Roots::None, Crown::Bush { radius: (1.4, 2.4) })
                },
                Tree::Dead => species("oak", "oak", (3, 6), false, 1.0, Roots::None, Crown::Dead { branches: (2, 4), reach: (1.5, 3.0) }),
                Tree::Cactus => Species { log: id("cactus"), ..species("oak", "oak", (1, 4), false, 0.0, Roots::None, Crown::None) },
            })
            .into();
        Self { species }
    }

    /// The leaves that colour a species' canopy from afar.
    pub fn leaves(&self, tree: Tree) -> BlockId {
        self.species[tree as usize].leaves
    }

    /// Grows a tree standing on the ground at `base` (the first block above it).
    pub fn grow(&self, tree: Tree, base: IVec3, rng: &mut Rng, blocks: &mut Blocks<'_>) {
        let s = &self.species[tree as usize];
        let mut g = Grower { blocks, seed: rng.next_u64(), base };
        let leaves = match s.alternate {
            Some((alternate, share)) if rng.chance(share) => alternate,
            _ => s.leaves,
        };
        let height = rng.range(s.height.0, s.height.1 + 1);
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
                let base_radius = pick(rng, radius);
                let first = (height as f32 * start) as i32;
                let span = (height + 2 - first) as f32;
                for y in first..=height + 1 {
                    let t = (y - first) as f32 / span;
                    // Whorls: every other layer steps in, so the cone has tiers.
                    let tier = if (height - y) % 2 == 0 { 1.0 } else { 0.65 };
                    let r = (base_radius * (1.0 - t) * tier).max(if y <= height { 0.8 } else { 0.0 });
                    g.disc(trunk(y as f32 / height as f32), r, leaves);
                }
                g.blob(top + Vec3::Y * 1.5, Vec3::new(0.5, 1.2, 0.5), leaves);
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
            Crown::None => {}
        }
    }
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
/// generated. Wood replaces leaves but leaves never replace wood, so the order in which
/// parts (or overlapping trees) are written does not matter.
struct Grower<'a, 'b> {
    blocks: &'a mut Blocks<'b>,
    /// Seeds per-voxel randomness (ragged leaf edges).
    seed: u64,
    base: IVec3,
}

impl Grower<'_, '_> {
    fn unit(&self, cell: IVec3) -> f32 {
        (hash3(self.seed, cell.x, cell.y, cell.z) & 0xffff) as f32 / 65536.0
    }

    fn reachable(&self, cell: IVec3) -> bool {
        (cell.x - self.base.x).abs() <= REACH && (cell.z - self.base.z).abs() <= REACH
    }

    fn wood(&mut self, cell: IVec3, log: BlockId) {
        if self.reachable(cell) {
            self.blocks.put_log(cell, log);
        }
    }

    fn leaf(&mut self, cell: IVec3, leaves: BlockId) {
        if self.reachable(cell) {
            self.blocks.put_if_air(cell, leaves);
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
                    } else if self.reachable(cell) {
                        self.blocks.put_if_air(cell, log);
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
                        self.blocks.put_if_air(cell, leaves);
                    }
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
