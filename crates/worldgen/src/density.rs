//! The land's 3D density (docs/terrain-foundation.md, section 6.3): where the router's
//! `crag` field asks for it, a 3D noise moves the ground by up to that many blocks, so the
//! ranges break into crags, ledges, buttresses and overhangs instead of smooth slopes.
//!
//! Above a column's preliminary surface `s` (the router's height, with rivers cut in) the
//! density is `(s − y) · above + a · n`, below it `(s − y) + a · n`, where `a` is the crag
//! amplitude in blocks and `n` a 3D noise in −1 ..= 1; the ground is solid where the
//! density is at least zero. Density falls more slowly above the surface than below it (as
//! vanilla's does, by its "quarter negative"), so rock reaches out over air more readily
//! than air bites into rock. Below `a` blocks under the surface and above `a / above` over
//! it the noise cannot change anything, so only that band is ever sampled, and land with no
//! crag (plains, valleys, coasts, near water) costs nothing.
//!
//! The noise lies on a lattice 4 blocks across and `STEP_Y` up, interpolated, the same for
//! whole columns during generation (`crate::lattice::Lattice`) and for single positions
//! (`Density::tops`): a column's top is the same however it is asked for.

use fastnoise_lite::{FastNoiseLite, FractalType};
use rustc_hash::FxHashMap;

use crate::content::DensityDef;
use crate::lattice::{self, Profile};

/// Vertical spacing of the density lattice, in blocks (a power of two).
pub const STEP_Y: i32 = 8;

/// What a column's density is made from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Crag {
    /// The preliminary surface, in blocks: the router's height with rivers cut in.
    pub surface: f32,
    /// How far the noise moves the ground, in blocks; 0 where it stays as it is.
    pub amplitude: f32,
    /// At and below this height the ground is solid whatever the noise: the water beside
    /// it, and the sea, never meet a hollow.
    pub floor: i32,
}

impl Crag {
    /// The heights the noise can change, from the lowest to the highest (empty when it
    /// changes none).
    pub fn band(&self, above: f32) -> (i32, i32) {
        let low = ((self.surface - self.amplitude).floor() as i32).max(self.floor + 1);
        let high = (self.surface + self.amplitude / above).ceil() as i32;
        (low, high)
    }

    /// Whether the block at height `y`, where the noise is `n`, is solid.
    #[inline]
    pub fn solid(&self, y: i32, n: f32, above: f32) -> bool {
        if y <= self.floor {
            return true;
        }
        let depth = self.surface - y as f32;
        let d = if depth < 0.0 { depth * above } else { depth };
        d + self.amplitude * n >= 0.0
    }
}

pub struct Density {
    pub(crate) noise: FastNoiseLite,
    /// Heights are multiplied by this before sampling: above 1, the noise changes faster up
    /// than across, so rock breaks into ledges.
    pub(crate) squash: f32,
    /// How much more slowly density falls above the surface than below.
    pub(crate) above: f32,
}

impl Density {
    pub fn new(seed: u64, def: &DensityDef) -> Self {
        Self {
            noise: crate::geology::noise(seed, 50, 1.0 / def.wavelength, FractalType::FBm, def.octaves),
            squash: def.squash,
            above: def.above.max(0.05),
        }
    }

    /// The top block of each column at (x, z) with its crag: the highest solid block,
    /// exactly as generation fills it. Columns without a crag keep the whole part of their
    /// surface. The lattice points the columns share are sampled once.
    pub fn tops(&self, columns: &[(i32, i32, Crag)]) -> Vec<i32> {
        let step = lattice::STEP;
        let mut index: FxHashMap<(i32, i32, i32), u32> = FxHashMap::default();
        let mut points: Vec<[f32; 3]> = Vec::new();
        let mut needs = Vec::with_capacity(columns.len());
        for &(x, z, crag) in columns {
            if crag.amplitude <= 0.0 {
                needs.push(None);
                continue;
            }
            let (low, high) = crag.band(self.above);
            if low > high {
                needs.push(None);
                continue;
            }
            let (first, last) = (low.div_euclid(STEP_Y), high.div_euclid(STEP_Y) + 1);
            let (cx, cz) = (x - x.rem_euclid(step), z - z.rem_euclid(step));
            for layer in first..=last {
                for (dx, dz) in [(0, 0), (step, 0), (0, step), (step, step)] {
                    index.entry((cx + dx, cz + dz, layer)).or_insert_with(|| {
                        points.push([(cx + dx) as f32, (layer * STEP_Y) as f32 * self.squash, (cz + dz) as f32]);
                        (points.len() - 1) as u32
                    });
                }
            }
            needs.push(Some((low, high, first, cx, cz)));
        }
        let mut values = vec![0.0f32; points.len()];
        crate::noise::noise_3d(&self.noise, &points, &mut values);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        columns
            .iter()
            .zip(needs)
            .map(|(&(x, z, crag), need)| {
                let Some((low, high, first, cx, cz)) = need else { return crag.surface.floor() as i32 };
                let (fx, fz) = (x.rem_euclid(step) as f32 / step as f32, z.rem_euclid(step) as f32 / step as f32);
                let at = |dx: i32, dz: i32, layer: i32| values[index[&(cx + dx, cz + dz, layer)] as usize];
                // The column's values layer by layer, as `Lattice::profile` interpolates them.
                let layers: Vec<f32> = (first..=high.div_euclid(STEP_Y) + 1)
                    .map(|layer| lerp(lerp(at(0, 0, layer), at(step, 0, layer), fx), lerp(at(0, step, layer), at(step, step, layer), fx), fz))
                    .collect();
                let noise = |y: i32| {
                    let layer = (y.div_euclid(STEP_Y) - first) as usize;
                    let t = y.rem_euclid(STEP_Y) as f32 * (1.0 / STEP_Y as f32);
                    lerp(layers[layer], layers[layer + 1], t)
                };
                // Below the band the ground is solid, so the first solid block from its top
                // down is the column's top.
                (low..=high).rev().find(|&y| crag.solid(y, noise(y), self.above)).unwrap_or(low - 1)
            })
            .collect()
    }

    /// Which heights of a column within `low..=high` are hollow: `out[i]` for height
    /// `low + i`. `profile` holds the noise above the column.
    pub fn hollows(&self, crag: &Crag, profile: &Profile, low: i32, high: i32, out: &mut Vec<bool>) {
        out.clear();
        out.extend((low..=high).map(|y| !crag.solid(y, profile.at(y), self.above)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::glam::IVec3;

    /// A column's top is the same block whichever way it is worked out, and every block
    /// above it is air while it is solid.
    #[test]
    fn tops_match_the_lattice() {
        let density = Density::new(3, &DensityDef::default());
        let origin = IVec3::new(64, 0, -96);
        let crag = |x: i32, z: i32| Crag { surface: 150.0 + 0.7 * x as f32 - 0.3 * z as f32, amplitude: 4.0 + (x % 7) as f32, floor: 130 };
        let lattice = crate::lattice::Lattice::noise(&density.noise, origin, 100, 250, STEP_Y, density.squash);
        let mut profile = Profile::default();
        let columns: Vec<(i32, i32, Crag)> = (0..32 * 32).map(|i| (origin.x + i % 32, origin.z + i / 32, crag(i % 32, i / 32))).collect();
        let tops = density.tops(&columns);
        let alone: Vec<i32> = columns.iter().map(|&c| density.tops(&[c])[0]).collect();
        assert_eq!(tops, alone);
        for (&(x, z, crag), &top) in columns.iter().zip(&tops) {
            lattice.profile(x - origin.x, z - origin.z, &mut profile);
            assert!(crag.solid(top, profile.at(top), density.above), "({x}, {z}) top {top}");
            let (_, high) = crag.band(density.above);
            assert!((top + 1..=high).all(|y| !crag.solid(y, profile.at(y), density.above)), "({x}, {z}) above {top}");
        }
    }
}
