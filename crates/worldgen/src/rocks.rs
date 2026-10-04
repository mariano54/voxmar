//! Rock provinces: which stones lie beneath, independent of the climate.
//!
//! The world is divided into cells about three kilometres across on a jittered grid, each
//! cell a province drawn by weight and rarity from `rocks.ron`. Borders are pushed around by
//! a slow warp carried on the climate lattice, so they wander instead of running straight,
//! and finding a position's province costs nine hashes.

use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::content::WorldContent;
use crate::rarity::{Featured, Rarity};
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
        let mut best = (f32::MAX, 0u64);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let h = hash3(self.seed, cx + dx, 0, cz + dz);
                let jitter = |bits: u64| 0.1 + 0.8 * (bits & 0xffff) as f32 / 65536.0;
                let (ox, oz) = ((cx + dx) as f32 + jitter(h), (cz + dz) as f32 + jitter(h >> 16));
                let d = (ox - px).powi(2) + (oz - pz).powi(2);
                if d < best.0 {
                    best = (d, h);
                }
            }
        }
        let roll = (best.1 >> 40) as f32 / (1u64 << 24) as f32;
        ProvinceId(self.shares.iter().position(|&s| roll < s).unwrap_or(self.shares.len() - 1) as u8)
    }
}
