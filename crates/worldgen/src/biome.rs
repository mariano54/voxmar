//! Biomes: data definitions placed over the continuous climate.
//!
//! The picker reads smooth fields only (temperature at the regional elevation, moisture,
//! continentalness, the landform class, weirdness, aether and the variant field), so
//! borders are smooth curves that follow climate and landforms, never noise or slope.
//!
//! Four axes are quantised into the named bands of `climate.ron`, and every cell of that
//! table belongs to at least one land biome (checked at load), so no climate is left to a
//! guess (Terralith's table). Biomes sharing a cell split it along the evenly spread
//! weirdness field: common ones by weight in the middle of its range, rarer ones in its
//! extremes, at a share set by their rarity. Weirdness varies over kilometres, so a rare
//! biome appears as whole regions in some places and not at all in others. A claim with an
//! aether range displaces the ordinary biomes of its cells wherever aether is that high.
//!
//! Each biome then gives up shares of its land to its variants, by the evenly spread
//! variant field (slots ordered by name hash, so adding one never moves the others), and
//! ecotones take thin slices along the borders of the biomes they sit between.
//!
//! Picks are made per 4×4-block cell. Each also measures the distance to the nearest
//! border analytically: for every boundary near the point in field space, the field's
//! distance to it over the field's gradient gives blocks, and a probe across it tells
//! whether the biome changes there and into what. `Site` carries that distance and the
//! biome next door, so borders blend (within 16–48 blocks, discrete things dither between
//! the two biomes) and later stages (shores, dunes, shapes) can read them.

use rustc_hash::FxHashMap;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::IVec2;

use crate::content::{Axis, BiomeDef, Colors, Layer, Role, Stones, WorldContent};
use crate::geology::{smoothstep, spline};
use crate::rarity::{Featured, Rarity, Tier, name_hash};
use crate::rocks::{ProvinceId, Rocks};
use crate::surface::{Palette, Surface, SurfaceRules};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BiomeId(pub u16);

/// A biome compiled for one world.
pub struct Biome {
    pub name: String,
    pub tags: Vec<String>,
    pub layer: Layer,
    pub role: Role,
    pub rarity: Rarity,
    /// Colours this biome sets for itself, over the climate colours.
    pub colors: Colors,
    pub(crate) surface: Surface,
    /// The biome's own stones; the rock province's when `None`.
    pub(crate) stones: Option<Palette>,
    pub(crate) strata: Vec<BlockId>,
    /// The centre and half-width of its first climate box on each table axis, where
    /// bounded.
    centre: [Option<(f32, f32)>; 4],
}

impl Biome {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

/// Picker axes: the four table axes, then weirdness, aether and variant.
const T: usize = 0;
const M: usize = 1;
const C: usize = 2;
const E: usize = 3;
const W: usize = 4;
const A: usize = 5;
const V: usize = 6;
const AXES: [Axis; 4] = [Axis::Temperature, Axis::Moisture, Axis::Continentalness, Axis::Erosion];

/// The fields a biome is picked by at one position, in axis order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point(pub [f32; 7]);

/// Farthest a border is looked for, in blocks.
pub const EDGE_HORIZON: f32 = 128.0;
/// Half the span of the central differences that give the fields' gradients, in blocks:
/// wide enough to smooth over the kinks of the climate lattice's interpolation.
pub(crate) const GRADIENT_STEP: i32 = 16;
/// Biomes are picked once per cell this many blocks square.
pub const CELL: i32 = 4;

/// One biome's claim on a cell of the table.
#[derive(Clone, Copy, Debug)]
struct Claim {
    biome: BiomeId,
    aether: Option<(f32, f32)>,
    /// Weight among the commons, or the share of the cell's weirdness range for rarer ones.
    share: f32,
    common: bool,
}

/// Where an underground claim holds within its cell, beyond aether.
#[derive(Clone, Copy, Debug)]
struct Gate {
    depth: (f32, f32),
    height: (f32, f32),
    /// A bit per rock province it lies in.
    provinces: u64,
}

impl Gate {
    fn holds(&self, depth: f32, height: f32, province: ProvinceId) -> bool {
        (self.depth.0..self.depth.1).contains(&depth) && (self.height.0..self.height.1).contains(&height) && self.provinces & 1 << province.0 != 0
    }
}

/// A variant's slot along the variant field.
#[derive(Clone, Copy, Debug)]
struct Slot {
    start: f32,
    end: f32,
    biome: BiomeId,
}

/// A biome on the border of two others, along one axis.
#[derive(Clone, Copy, Debug)]
struct Ecotone {
    axis: usize,
    half_width: f32,
    biome: BiomeId,
}

/// What the picker decided for a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    /// The biome the cell belongs to.
    pub home: BiomeId,
    /// The nearest other biome, across the nearest border; `home` if none is near.
    pub neighbour: BiomeId,
    /// Distance to that border in blocks, at most `EDGE_HORIZON`.
    pub edge: f32,
    /// 0 at the centre of the home biome's climate ..= 1 at its edge.
    pub from_centre: f32,
    /// The biome of rivers and lakes in this climate.
    pub river: Option<BiomeId>,
}

pub struct Biomes {
    list: Vec<Biome>,
    /// Band edges per table axis.
    edges: [Vec<f32>; 4],
    remaps: [Option<Vec<(f32, f32)>>; 4],
    counts: [usize; 4],
    /// Land claims per cell of the table: rarer ones first, rarest first, then commons.
    land: Vec<Vec<Claim>>,
    /// River and lake claims per cell.
    rivers: Vec<Vec<Claim>>,
    /// Underground claims per cell, in the same order as land claims.
    underground: Vec<Vec<(Claim, Gate)>>,
    variants: Vec<Vec<Slot>>,
    ecotones: FxHashMap<(BiomeId, BiomeId), Ecotone>,
    /// Ecotone half-widths along temperature and moisture, for finding the slices' edges.
    ecotone_widths: [Vec<f32>; 2],
}

impl Biomes {
    pub fn new(content: &WorldContent, registry: &BlockRegistry, rules: &mut SurfaceRules, rocks: &Rocks, featured: &Featured) -> Self {
        let defs: Vec<&BiomeDef> = content.biomes.iter().collect();
        let id = |name: &str| BiomeId(defs.iter().position(|b| b.name == name).expect("checked at load") as u16);
        let named: Vec<(&str, Rarity)> = defs.iter().map(|b| (b.name.as_str(), b.rarity)).collect();
        let boost = featured.multipliers("biome", &named);
        let share = |i: usize| defs[i].rarity.multiplier() * boost[i];

        let edges = AXES.map(|axis| content.bands(axis).edges.clone());
        let counts = AXES.map(|axis| content.bands(axis).names.len());
        let band = |axis: Axis, name: &str| content.bands(axis).names.iter().position(|n| n == name).expect("checked at load");

        let list: Vec<Biome> = defs
            .iter()
            .map(|def| Biome {
                name: def.name.clone(),
                tags: def.tags.clone(),
                layer: def.layer,
                role: def.role,
                rarity: def.rarity,
                colors: def.colors.clone(),
                // Overworld ground keeps to its colours; caves keep their contrasts.
                surface: rules.compile(&def.surface, registry, rocks, (def.layer == Layer::Surface).then(|| crate::hue::biome_tints(content, def)[0])),
                stones: match &def.stones {
                    Stones::Province => None,
                    Stones::Blocks(selects) => Some(Palette::new(selects, registry)),
                },
                strata: def.strata.iter().map(|name| registry.expect(name)).collect(),
                centre: std::array::from_fn(|a| {
                    let (from, to) = def.climate.first()?.bands(AXES[a])?;
                    let (lo, hi) = (band(AXES[a], from), band(AXES[a], to));
                    let lower = band_edge(&edges[a], lo as isize - 1);
                    let upper = band_edge(&edges[a], hi as isize);
                    Some(((lower + upper) / 2.0, (upper - lower) / 2.0))
                }),
            })
            .collect();

        let cells = counts.iter().product();
        let (mut land, mut rivers, mut underground) = (vec![Vec::new(); cells], vec![Vec::new(); cells], vec![Vec::new(); cells]);
        let any = (f32::NEG_INFINITY, f32::INFINITY);
        for (i, def) in defs.iter().enumerate() {
            let common = def.rarity == Rarity::Common;
            for climate in &def.climate {
                let ranges = AXES.map(|axis| match climate.bands(axis) {
                    Some((from, to)) => band(axis, from)..=band(axis, to),
                    None => 0..=counts[axis as usize] - 1,
                });
                let claim = Claim { biome: BiomeId(i as u16), aether: climate.aether, share: def.weight * if common { 1.0 } else { share(i) }, common };
                let provinces = climate.provinces.as_ref().map_or(u64::MAX, |names| {
                    (0..rocks.len().min(64)).filter(|&p| names.iter().any(|n| rocks.matches(ProvinceId(p as u8), n))).fold(0, |m, p| m | 1 << p)
                });
                let gate = Gate { depth: climate.depth.unwrap_or(any), height: climate.height.unwrap_or(any), provinces };
                for t in ranges[0].clone() {
                    for m in ranges[1].clone() {
                        for c in ranges[2].clone() {
                            for e in ranges[3].clone() {
                                let index = ((t * counts[1] + m) * counts[2] + c) * counts[3] + e;
                                match (def.layer, def.role) {
                                    (Layer::Underground, _) => underground[index].push((claim, gate)),
                                    (Layer::Surface, role) => {
                                        let cell: &mut Vec<Claim> = if role == Role::River { &mut rivers[index] } else { &mut land[index] };
                                        if !cell.iter().any(|other| other.biome == claim.biome && other.aether == claim.aether) {
                                            cell.push(claim);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // Rarer claims first, rarest at the front; then commons, all in name-hash order.
        let hash = |claim: &Claim| name_hash(&defs[claim.biome.0 as usize].name);
        let order = |a: &Claim, b: &Claim| {
            let rarity = |c: &Claim| if c.common { f32::INFINITY } else { c.share };
            a.common.cmp(&b.common).then(rarity(a).total_cmp(&rarity(b))).then(hash(a).cmp(&hash(b)))
        };
        for cell in land.iter_mut().chain(rivers.iter_mut()) {
            cell.sort_by(order);
        }
        for cell in &mut underground {
            cell.sort_by(|a, b| order(&a.0, &b.0));
        }

        let variants = defs
            .iter()
            .map(|def| {
                let mut slots: Vec<(u64, BiomeId, f32)> = def
                    .variants
                    .iter()
                    .map(|(name, area)| {
                        let v = id(name);
                        (name_hash(name), v, area * share(v.0 as usize))
                    })
                    .collect();
                // The smallest take the ends of the variant field, alternating, so they lie
                // in whole patches around its highs and lows rather than in thin rings
                // around a middle value; larger ones lie next to them.
                slots.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
                let scale = 1.0 / slots.iter().map(|s| s.2).sum::<f32>().max(1.0);
                let (mut bottom, mut top) = (0.0f32, 1.0f32);
                slots
                    .into_iter()
                    .enumerate()
                    .map(|(i, (_, biome, area))| {
                        let width = area * scale;
                        if i % 2 == 0 {
                            bottom += width;
                            Slot { start: bottom - width, end: bottom, biome }
                        } else {
                            top -= width;
                            Slot { start: top, end: top + width, biome }
                        }
                    })
                    .collect()
            })
            .collect();

        let mut ecotones = FxHashMap::default();
        let mut ecotone_widths = [Vec::new(), Vec::new()];
        for (i, def) in defs.iter().enumerate() {
            if let Some(e) = &def.ecotone {
                let axis = if e.axis == Axis::Temperature { T } else { M };
                let ecotone = Ecotone { axis, half_width: e.width / 2.0, biome: BiomeId(i as u16) };
                let (a, b) = (id(&e.between.0), id(&e.between.1));
                ecotones.insert((a, b), ecotone);
                ecotones.insert((b, a), ecotone);
                ecotone_widths[axis].push(e.width / 2.0);
            }
        }

        let remaps = AXES.map(|axis| content.climate.remaps.iter().find(|r| r.axis == axis).map(|r| r.points.clone()));
        Self { list, edges, remaps, counts, land, rivers, underground, variants, ecotones, ecotone_widths }
    }

    pub fn get(&self, id: BiomeId) -> &Biome {
        &self.list[id.0 as usize]
    }

    /// Every biome's strata.
    pub(crate) fn strata(&self) -> impl Iterator<Item = &[BlockId]> {
        self.list.iter().map(|biome| biome.strata.as_slice())
    }

    pub fn id(&self, name: &str) -> Option<BiomeId> {
        self.list.iter().position(|b| b.name == name).map(|i| BiomeId(i as u16))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (BiomeId, &Biome)> {
        self.list.iter().enumerate().map(|(i, b)| (BiomeId(i as u16), b))
    }

    /// Applies the picker-only remaps to the four table axes.
    pub(crate) fn remap(&self, mut point: Point) -> Point {
        for (axis, remap) in self.remaps.iter().enumerate() {
            if let Some(points) = remap {
                point.0[axis] = spline(points, point.0[axis]);
            }
        }
        point
    }

    /// The table cell holding a point.
    fn cell(&self, p: &Point) -> usize {
        let band = |a: usize| self.edges[a].partition_point(|&edge| edge <= p.0[a]);
        ((band(T) * self.counts[1] + band(M)) * self.counts[2] + band(C)) * self.counts[3] + band(E)
    }

    /// The claimant of a cell at this aether and weirdness, and the span of weirdness it
    /// holds there.
    fn claimant(claims: &[Claim], aether: f32, weirdness: f32) -> Option<(BiomeId, f32, f32)> {
        let valid = |c: &Claim| c.aether.is_none_or(|(lo, hi)| lo <= aether && aether < hi);
        // Where aether lets a fantastical common claim in, it displaces the ordinary ones.
        let fantastical = claims.iter().any(|c| c.common && c.aether.is_some() && valid(c));
        let common = |c: &Claim| c.common && valid(c) && c.aether.is_some() == fantastical;
        let weight: f32 = claims.iter().filter(|c| common(c)).map(|c| c.share).sum();
        // Without a common claimant, the least rare of the rest fills the cell.
        let filler = if weight > 0.0 { None } else { claims.iter().rev().find(|c| !c.common && valid(c)).map(|c| c.biome) };
        let rare = |c: &Claim| !c.common && valid(c) && Some(c.biome) != filler;
        let rare_total: f32 = claims.iter().filter(|c| rare(c)).map(|c| c.share).sum();
        let scale = if rare_total > 0.5 { 0.5 / rare_total } else { 1.0 };

        // Rarer claimants take the extremes, alternating ends, the rarest outermost.
        let (mut bottom, mut top) = (0.0f32, 1.0f32);
        for (i, claim) in claims.iter().filter(|c| rare(c)).enumerate() {
            let width = claim.share * scale;
            let (lo, hi) = if i % 2 == 0 { (bottom, bottom + width) } else { (top - width, top) };
            if (lo..hi).contains(&weirdness) {
                return Some((claim.biome, lo, hi));
            }
            if i % 2 == 0 {
                bottom = hi;
            } else {
                top = lo;
            }
        }
        if let Some(filler) = filler {
            return Some((filler, bottom, top));
        }
        // The commons share the whole range, the rarer claims laid over its ends, so a
        // common's slice is the same in every cell with the same commons whatever rarer
        // claims (and aether) let in: their borders carry on across the cells.
        let mut lo = 0.0;
        let mut last = None;
        for claim in claims.iter().filter(|c| common(c)) {
            let hi = lo + claim.share / weight;
            if weirdness < hi {
                return Some((claim.biome, lo.max(bottom), hi.min(top)));
            }
            last = Some((claim.biome, lo.max(bottom), top));
            lo = hi;
        }
        last
    }

    /// The land biome a point belongs to before variants and ecotones.
    fn parent(&self, p: &Point) -> BiomeId {
        Self::claimant(&self.land[self.cell(p)], p.0[A], p.0[W]).map_or(BiomeId(0), |(biome, _, _)| biome)
    }

    /// The biome a point belongs to.
    pub(crate) fn resolve(&self, p: &Point) -> BiomeId {
        let parent = self.parent(p);
        if let Some(ecotone) = self.ecotone(p, parent) {
            return ecotone;
        }
        self.variant(parent, p.0[V])
    }

    /// The biome a variant belongs to; any other biome itself.
    pub fn parent_of(&self, id: BiomeId) -> BiomeId {
        (0..self.variants.len()).find(|&p| self.variants[p].iter().any(|s| s.biome == id)).map_or(id, |p| BiomeId(p as u16))
    }

    /// The band index of a point along each of the four table axes.
    #[cfg(test)]
    pub(crate) fn bands(&self, p: &Point) -> [u8; 4] {
        std::array::from_fn(|a| self.edges[a].partition_point(|&edge| edge <= p.0[a]) as u8)
    }

    fn variant(&self, parent: BiomeId, v: f32) -> BiomeId {
        self.variants[parent.0 as usize].iter().find(|s| (s.start..s.end).contains(&v)).map_or(parent, |s| s.biome)
    }

    /// The ecotone a point lies in: within half an ecotone's width of a band edge, where
    /// the land across the edge belongs to the ecotone's other parent.
    fn ecotone(&self, p: &Point, parent: BiomeId) -> Option<BiomeId> {
        for axis in [T, M] {
            let widest = self.ecotone_widths[axis].iter().copied().fold(0.0, f32::max);
            if widest == 0.0 {
                continue;
            }
            let value = p.0[axis];
            let edges = &self.edges[axis];
            let i = edges.partition_point(|&e| e <= value);
            for edge in [i.checked_sub(1).map(|j| edges[j]), edges.get(i).copied()].into_iter().flatten() {
                let gap = (value - edge).abs();
                if gap >= widest {
                    continue;
                }
                let mut across = *p;
                across.0[axis] = edge + if value < edge { 1e-3 } else { -1e-3 };
                if let Some(ecotone) = self.ecotones.get(&(parent, self.parent(&across)))
                    && ecotone.axis == axis
                    && gap < ecotone.half_width
                {
                    return Some(ecotone.biome);
                }
            }
        }
        None
    }

    /// The river or lake biome of a point's climate, if any claims it.
    pub(crate) fn river(&self, p: &Point) -> Option<BiomeId> {
        Self::claimant(&self.rivers[self.cell(p)], p.0[A], p.0[W]).map(|(biome, _, _)| biome)
    }

    /// The underground biome at `p`, `depth` blocks below the surface and `height` blocks
    /// above sea level (negative below it), in rock province `province`. The weirdness and
    /// variant of `p` are the underground's own 3D fields. `None` where no underground
    /// biome claims the place.
    pub(crate) fn underground(&self, p: &Point, depth: f32, height: f32, province: ProvinceId) -> Option<BiomeId> {
        const MOST: usize = 24;
        let mut claims = [Claim { biome: BiomeId(0), aether: None, share: 0.0, common: false }; MOST];
        let mut n = 0;
        for (claim, gate) in &self.underground[self.cell(p)] {
            if n < MOST && gate.holds(depth, height, province) {
                claims[n] = *claim;
                n += 1;
            }
        }
        let (biome, _, _) = Self::claimant(&claims[..n], p.0[A], p.0[W])?;
        Some(self.variant(biome, p.0[V]))
    }

    /// Picks the biome at `p` and finds its nearest border, given the points
    /// `GRADIENT_STEP` blocks either side of it along x and along z: `[+x, −x, +z, −z]`.
    pub(crate) fn pick(&self, p: &Point, around: &[Point; 4]) -> Pick {
        let home = self.resolve(p);
        let span = 2.0 * GRADIENT_STEP as f32;
        let gradient: [f32; 7] = std::array::from_fn(|a| (around[0].0[a] - around[1].0[a]).hypot(around[2].0[a] - around[3].0[a]) / span);

        // Every boundary within reach, nearest first; the nearest across which the biome
        // changes is the border. Most cells are far from all of them and probe none.
        let mut candidates = [(0.0f32, 0u8, 0.0f32); 96];
        let mut count = 0;
        let mut consider = |axis: usize, boundary: f32| {
            let distance = (p.0[axis] - boundary).abs() / gradient[axis].max(1e-9);
            if distance < EDGE_HORIZON && count < candidates.len() {
                candidates[count] = (distance, axis as u8, boundary);
                count += 1;
            }
        };

        // Band edges, and the edges of ecotone slices around them. Every edge within reach
        // counts, not just the nearest two, so the distance stays continuous where the
        // nearest edge divides nothing.
        for axis in [T, M, C, E] {
            for &boundary in &self.edges[axis] {
                consider(axis, boundary);
                if axis <= M {
                    for &half in &self.ecotone_widths[axis] {
                        consider(axis, boundary - half);
                        consider(axis, boundary + half);
                    }
                }
            }
        }
        // The split of the cell along weirdness, and aether ranges.
        let claims = &self.land[self.cell(p)];
        if let Some((_, lo, hi)) = Self::claimant(claims, p.0[A], p.0[W]) {
            consider(W, lo);
            consider(W, hi);
        }
        for claim in claims {
            if let Some((lo, hi)) = claim.aether {
                consider(A, lo);
                consider(A, hi);
            }
        }
        // Variant slots.
        for slot in &self.variants[self.parent(p).0 as usize] {
            consider(V, slot.start);
            consider(V, slot.end);
        }

        let candidates = &mut candidates[..count];
        candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let (mut edge, mut neighbour) = (EDGE_HORIZON, home);
        for &(distance, axis, boundary) in candidates.iter() {
            let axis = axis as usize;
            let mut across = *p;
            let nudge = if axis == T { 1e-3 } else { 1e-5 };
            across.0[axis] = boundary + if p.0[axis] < boundary { nudge } else { -nudge };
            let other = self.resolve(&across);
            if other != home {
                (edge, neighbour) = (distance, other);
                break;
            }
        }

        Pick { home, neighbour, edge, from_centre: self.centrality(home, p), river: self.river(p) }
    }

    fn centrality(&self, home: BiomeId, p: &Point) -> f32 {
        self.get(home)
            .centre
            .iter()
            .enumerate()
            .filter_map(|(a, c)| c.map(|(centre, half)| (p.0[a] - centre).abs() / half))
            .fold(0.0, f32::max)
            .min(1.0)
    }

    /// Share of the neighbouring biome at a distance `edge` from the border: a half on the
    /// border, nothing a blend width inside. The width runs from 16 to 48 blocks with
    /// `softness` (0 ..= 1), so some borders are crisp and others wide.
    pub fn blend(edge: f32, softness: f32) -> f32 {
        0.5 * (1.0 - smoothstep(0.0, 16.0 + 32.0 * softness, edge))
    }

    /// Searches outwards from `from`, on a square spiral of points `step` blocks apart, for
    /// the nearest one within `radius` blocks that belongs to `biome`. `point` gives the
    /// picker's fields at a position, and `confirm` checks a candidate the way sites are
    /// picked, which near a border can differ.
    pub(crate) fn locate(
        &self,
        biome: BiomeId,
        from: IVec2,
        radius: i32,
        step: i32,
        point: impl Fn(IVec2) -> Point,
        confirm: impl Fn(IVec2) -> bool,
    ) -> Option<IVec2> {
        for ring in 0..=radius / step {
            for i in 0..(ring * 8).max(1) {
                let (dx, dz) = crate::spiral(ring, i);
                let at = from + IVec2::new(dx, dz) * step;
                if self.resolve(&point(at)) == biome && confirm(at) {
                    return Some(at);
                }
            }
        }
        None
    }
}

/// The value of band edge `i` along an axis, extending the outermost bands by the width of
/// their neighbours.
pub(crate) fn band_edge(edges: &[f32], i: isize) -> f32 {
    let n = edges.len() as isize;
    let width = |a: isize, b: isize| if n >= 2 { edges[b as usize] - edges[a as usize] } else { 1.0 };
    match i {
        i if i < 0 => edges[0] - width(0, 1.min(n - 1)),
        i if i >= n => edges[(n - 1) as usize] + width((n - 2).max(0), n - 1),
        i => edges[i as usize],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    fn biomes() -> Biomes {
        let registry = BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap();
        let content = WorldContent::base(&registry).unwrap();
        let mut rules = SurfaceRules::new(1, &content, &registry);
        let featured = Featured::new(1, 0.5);
        let rocks = Rocks::new(1, &content, &registry, &featured);
        Biomes::new(&content, &registry, &mut rules, &rocks, &featured)
    }

    fn point(t: f32, m: f32, c: f32, e: f32, w: f32, a: f32, v: f32) -> Point {
        Point([t, m, c, e, w, a, v])
    }

    #[test]
    fn every_climate_has_a_land_biome() {
        let biomes = biomes();
        let mut rng = Rng::new(1, 2, 3, 4);
        for _ in 0..100_000 {
            let mut unit = || rng.unit();
            let p = point(-40.0 + 80.0 * unit(), 3.0 * unit(), -1.0 + 2.0 * unit(), unit(), unit(), unit(), unit());
            let biome = biomes.get(biomes.resolve(&p));
            assert!(biome.layer == Layer::Surface && biome.role == Role::Land, "{p:?} gives {}", biome.name);
            assert!(Biomes::claimant(&biomes.land[biomes.cell(&p)], p.0[A], p.0[W]).is_some(), "no claimant at {p:?}");
        }
    }

    #[test]
    fn rare_biomes_take_their_share_in_the_extremes() {
        let biomes = biomes();
        let glacier = biomes.id("base:glacier").unwrap();
        // A frozen, flat cell: tundra in the middle of weirdness, glacier at one end.
        let at = |w: f32| biomes.resolve(&point(-10.0, 1.0, 0.4, 0.0, w, 0.0, 0.5));
        let glacial: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).filter(|&w| at(w) == glacier).collect();
        let share = glacial.len() as f32 / 1000.0;
        // Rare is 0.08, possibly boosted a tier or two by the world's featured set.
        assert!((0.05..=0.35).contains(&share), "glacier share {share}");
        assert!(glacial.iter().all(|&w| w < 0.4) || glacial.iter().all(|&w| w > 0.6), "glacier not in an extreme: {glacial:?}");
        assert_ne!(at(0.5), glacier);
    }

    #[test]
    fn edges_point_at_the_neighbouring_biome() {
        let biomes = biomes();
        // Temperature rises 0.01 °C a block along x across the cold/cool edge at 4 °C,
        // between taiga and plains.
        // The variant field's middle, where no variant lies.
        let at = |x: f32| point(3.0 + 0.01 * x, 0.75, 0.4, 0.0, 0.5, 0.0, 0.5);
        let step = GRADIENT_STEP as f32;
        let pick = |x: f32| biomes.pick(&at(x), &[at(x + step), at(x - step), at(x), at(x)]);
        let near = pick(60.0);
        assert!((near.edge - 40.0).abs() < 1.0, "{near:?}");
        assert_eq!(biomes.get(near.home).name, "base:taiga");
        assert_eq!(biomes.get(near.neighbour).name, "base:plains");
        let across = pick(130.0);
        assert_eq!((across.home, across.neighbour), (near.neighbour, near.home));
        assert!((across.edge - 30.0).abs() < 1.0, "{across:?}");
        assert_eq!(pick(-300.0).edge, EDGE_HORIZON);
    }

    #[test]
    fn legendary_biomes_are_a_real_find() {
        let registry = BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap();
        let mut files: Vec<_> = crate::content::BASE_FILES.iter().map(|(name, source)| voxmar_engine::content::ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() }).collect();
        // A legendary biome that may grow in every climate.
        files.push(voxmar_engine::content::ContentFile {
            package: "test".into(),
            name: "biomes.ron".into(),
            source: "(biomes: [(name: \"marvel\", rarity: Legendary, climate: [()], surface: (ground: (top: Block(\"base:calcite\")), underwater: (top: Block(\"base:sand\"))))])".into(),
        });
        let content = WorldContent::from_files(files.iter(), &registry).unwrap();
        let mut shares = Vec::new();
        for seed in 0..30 {
            let mut rules = SurfaceRules::new(seed, &content, &registry);
            let featured = Featured::new(seed, 0.5);
            let rocks = Rocks::new(seed, &content, &registry, &featured);
            let biomes = Biomes::new(&content, &registry, &mut rules, &rocks, &featured);
            let marvel = biomes.id("test:marvel").unwrap();
            let mut rng = Rng::new(seed, 5, 6, 7);
            let samples = 200_000;
            let found = (0..samples)
                .filter(|_| {
                    let mut unit = || rng.unit();
                    biomes.resolve(&point(-20.0 + 55.0 * unit(), 2.5 * unit(), -1.0 + 2.0 * unit(), unit(), unit(), unit(), unit())) == marvel
                })
                .count();
            shares.push(found as f32 / samples as f32);
        }
        let mean = shares.iter().sum::<f32>() / shares.len() as f32;
        // 0.2% of the land, or a few percent in the odd world that features it.
        assert!(shares.iter().all(|&s| s > 0.0 && s < 0.1), "{shares:?}");
        assert!(mean < 0.02, "legendary biomes cover {mean} of the land on average: {shares:?}");
        assert!(shares.iter().filter(|&&s| s < 0.005).count() >= 20, "{shares:?}");
    }

    #[test]
    fn ecotones_sit_on_the_border() {
        let biomes = biomes();
        // Humid: taiga below 4 °C, forest above (where the weirdness gives the cell to
        // forest rather than the other woods sharing it), birch taiga within a degree of
        // the edge.
        let w = (0..100).map(|i| i as f32 / 100.0).find(|&w| biomes.get(biomes.resolve(&point(5.5, 1.1, 0.4, 0.0, w, 0.0, 0.5))).name == "base:forest").unwrap();
        let at = |t: f32| biomes.get(biomes.resolve(&point(t, 1.1, 0.4, 0.0, w, 0.0, 0.5))).name.as_str();
        assert_eq!(at(2.5), "base:taiga");
        assert_eq!(at(3.5), "base:birch_taiga");
        assert_eq!(at(4.5), "base:birch_taiga");
        assert_eq!(at(5.5), "base:forest");
    }
}
