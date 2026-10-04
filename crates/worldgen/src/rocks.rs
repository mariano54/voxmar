//! Rock provinces: which stones lie beneath, independent of the climate.
//!
//! The world is divided into cells about three kilometres across on a jittered grid, each
//! cell a province drawn by weight and rarity from `rocks.ron`. Borders are pushed around by
//! a slow warp carried on the climate lattice, so they wander instead of running straight,
//! and finding a position's province costs nine hashes.

use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::content::WorldContent;
use crate::rarity::{Featured, Rarity, Tier};
use crate::rng::hash3;
use crate::surface::Palette;

/// Spacing of province centres, in blocks.
const CELL: f32 = 3000.0;
const PURPOSE: u64 = 0x50c4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProvinceId(pub u8);

pub struct Province {
    pub name: String,
    pub tags: Vec<String>,
    pub(crate) stones: Palette,
}

pub struct Rocks {
    provinces: Vec<Province>,
    /// Cumulative shares of the world, ending at 1.
    shares: Vec<f32>,
    seed: u64,
}

impl Rocks {
    pub fn new(seed: u64, content: &WorldContent, registry: &BlockRegistry, featured: &Featured) -> Self {
        let named: Vec<(&str, Rarity)> = content.provinces.iter().map(|p| (p.name.as_str(), p.rarity)).collect();
        let boost = featured.multipliers("province", &named);
        let weights: Vec<f32> = content.provinces.iter().zip(&boost).map(|(p, b)| p.weight * p.rarity.multiplier() * b).collect();
        let total: f32 = weights.iter().sum();
        let mut sum = 0.0;
        let shares = weights
            .iter()
            .map(|w| {
                sum += w / total;
                sum
            })
            .collect();
        let provinces = content
            .provinces
            .iter()
            .map(|p| Province { name: p.name.clone(), tags: p.tags.clone(), stones: Palette::new(&p.stones, registry) })
            .collect();
        Self { provinces, shares, seed: seed ^ PURPOSE }
    }

    pub fn len(&self) -> usize {
        self.provinces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.provinces.is_empty()
    }

    pub fn get(&self, id: ProvinceId) -> &Province {
        &self.provinces[id.0 as usize]
    }

    /// Whether a province is named (`granite` or `base:granite`) or tagged `name`.
    pub fn matches(&self, id: ProvinceId, name: &str) -> bool {
        let p = self.get(id);
        p.name == name || p.name.split_once(':').is_some_and(|(_, local)| local == name) || p.tags.iter().any(|t| t == name)
    }

    /// Every block a province uses as stone.
    pub fn stones(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.provinces.iter().flat_map(|p| p.stones.blocks())
    }

    /// The province at (x, z), whose borders are displaced by `warp` blocks.
    pub fn at(&self, x: i32, z: i32, warp: [f32; 2]) -> ProvinceId {
        let (px, pz) = ((x as f32 + warp[0]) / CELL, (z as f32 + warp[1]) / CELL);
        let (cx, cz) = (px.floor() as i32, pz.floor() as i32);
        let mut best = (f32::MAX, ProvinceId(0));
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (centre, province) = self.cell(cx + dx, cz + dz);
                let d = (centre[0] - px).powi(2) + (centre[1] - pz).powi(2);
                if d < best.0 {
                    best = (d, province);
                }
            }
        }
        best.1
    }

    /// The centre of a cell, in cells, and its province.
    fn cell(&self, cx: i32, cz: i32) -> ([f32; 2], ProvinceId) {
        let h = hash3(self.seed, cx, 0, cz);
        let jitter = |bits: u64| 0.1 + 0.8 * (bits & 0xffff) as f32 / 65536.0;
        let roll = (h >> 40) as f32 / (1u64 << 24) as f32;
        let province = self.shares.iter().position(|&s| roll < s).unwrap_or(self.shares.len() - 1);
        ([cx as f32 + jitter(h), cz as f32 + jitter(h >> 16)], ProvinceId(province as u8))
    }

    /// Where the provinces named or tagged `name` lie, for shapes that only rise over them.
    pub fn mask(&self, name: &str) -> ProvinceMask {
        ProvinceMask {
            rocks: Rocks { provinces: Vec::new(), shares: self.shares.clone(), seed: self.seed },
            matching: (0..self.len()).map(|i| self.matches(ProvinceId(i as u8), name)).collect(),
        }
    }
}

/// Some of a world's provinces, and how far inside them a position lies.
pub struct ProvinceMask {
    /// The world's province cells, without their stones.
    rocks: Rocks,
    matching: Vec<bool>,
}

impl ProvinceMask {
    /// Blocks from (x, z) to the nearest border between a matching province and another:
    /// positive inside a matching one, negative outside, with borders displaced by `warp`.
    /// Exact up to about a third of a cell (a kilometre) from the border.
    pub fn depth(&self, x: f32, z: f32, warp: [f32; 2]) -> f32 {
        let p = [(x + warp[0]) / CELL, (z + warp[1]) / CELL];
        let (cx, cz) = (p[0].floor() as i32, p[1].floor() as i32);
        let mut cells = [([0.0f32; 2], false); 25];
        let mut nearest = (f32::MAX, 0);
        for (i, cell) in cells.iter_mut().enumerate() {
            let (centre, province) = self.rocks.cell(cx + i as i32 % 5 - 2, cz + i as i32 / 5 - 2);
            *cell = (centre, self.matching[province.0 as usize]);
            let d = (centre[0] - p[0]).powi(2) + (centre[1] - p[1]).powi(2);
            if d < nearest.0 {
                nearest = (d, i);
            }
        }
        let (a, inside) = cells[nearest.1];
        // The nearest border with a cell on the other side: the bisector of the two centres.
        let border = cells
            .iter()
            .filter(|(_, m)| *m != inside)
            .map(|(b, _)| {
                let n = [b[0] - a[0], b[1] - a[1]];
                let len = n[0].hypot(n[1]);
                let middle = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                -((p[0] - middle[0]) * n[0] + (p[1] - middle[1]) * n[1]) / len
            })
            .fold(1.0f32, f32::min);
        border * CELL * if inside { 1.0 } else { -1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::block::BlockRegistry;

    #[test]
    fn province_depth_measures_to_the_border() {
        let registry = BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap();
        let content = WorldContent::base(&registry).unwrap();
        let rocks = Rocks::new(5, &content, &registry, &crate::rarity::Featured::new(5, 0.5));
        let mask = rocks.mask("karst");
        let karst = |x: i32, z: i32| rocks.matches(rocks.at(x, z, [0.0; 2]), "karst");
        let (mut inside, mut outside) = (0, 0);
        for i in 0..120 * 120 {
            let (x, z) = (i % 120 * 251 - 15000, i / 120 * 251 - 15000);
            let depth = mask.depth(x as f32, z as f32, [0.0; 2]);
            assert_eq!(depth > 0.0, karst(x, z), "at {x}, {z}: depth {depth}");
            if depth > 0.0 { inside += 1 } else { outside += 1 }
            // The border is no nearer than the depth says: a step short of it in any
            // direction stays in the same kind of province.
            if depth.abs() > 40.0 && depth.abs() < 900.0 {
                let short = depth.abs() - 20.0;
                for (dx, dz) in [(1.0f32, 0.0f32), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
                    let p = (x as f32 + dx * short, z as f32 + dz * short);
                    assert_eq!(karst(p.0 as i32, p.1 as i32), depth > 0.0, "at {x}, {z}: depth {depth} but the border is nearer");
                }
            }
        }
        assert!(inside > 0 && outside > 0);
    }
}
