//! Noise sampled on a coarse lattice over a column and interpolated in between.
//!
//! Terrain and caves need 3D noise at every block, which would cost far too much evaluated
//! directly. Smooth noise changes little over a few blocks, so sampling it every 4 blocks
//! across (and 4 or 8 up) and interpolating trilinearly looks the same at a small fraction
//! of the cost. Thin shapes keep their detail because callers interpolate the noise fields
//! and only then combine them (a tunnel is where two interpolated fields are both near
//! zero), never the other way round.
//!
//! Values are read one block column at a time: [`Lattice::profile`] interpolates every
//! layer horizontally once, after which each block costs a single linear step in y.

use fastnoise_lite::FastNoiseLite;
use voxmar_engine::coords::{CHUNK_SIZE, WORLD_HEIGHT};
use voxmar_engine::glam::IVec3;

/// Horizontal lattice spacing, in blocks.
pub const STEP: i32 = 4;
/// Lattice points along each horizontal side of a column, including the far edge, which
/// the neighbouring column shares.
const SIDE: usize = (CHUNK_SIZE / STEP + 1) as usize;
/// Most layers a profile can hold: the world's height at the finest vertical spacing.
const MAX_LAYERS: usize = (WORLD_HEIGHT / STEP + 2) as usize;

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// 3D values on a lattice over one column, between two heights.
pub struct Lattice {
    values: Vec<f32>,
    /// Index of the lowest layer (its height over `step_y`).
    first: i32,
    layers: usize,
    step_y: i32,
}

impl Lattice {
    /// Samples `f` at every lattice point of the column at `origin` whose layer is needed to
    /// interpolate heights `low..=high`, with layers `step_y` blocks apart.
    pub fn new(origin: IVec3, low: i32, high: i32, step_y: i32, mut f: impl FnMut(IVec3) -> f32) -> Self {
        let first = low.div_euclid(step_y);
        let layers = (high.div_euclid(step_y) - first + 2).max(2) as usize;
        debug_assert!(layers <= MAX_LAYERS);
        let mut values = Vec::with_capacity(SIDE * SIDE * layers);
        for layer in 0..layers as i32 {
            for lz in 0..SIDE as i32 {
                for lx in 0..SIDE as i32 {
                    values.push(f(origin + IVec3::new(lx * STEP, (first + layer) * step_y, lz * STEP)));
                }
            }
        }
        Self { values, first, layers, step_y }
    }

    /// A noise field, with heights multiplied by `scale_y` (above 1 flattens its shapes).
    pub fn noise(noise: &FastNoiseLite, origin: IVec3, low: i32, high: i32, step_y: i32, scale_y: f32) -> Self {
        Self::new(origin, low, high, step_y, |p| noise.get_noise_3d(p.x as f32, p.y as f32 * scale_y, p.z as f32))
    }

    /// The values above the block column at (x, z), relative to the column origin.
    pub fn profile(&self, x: i32, z: i32, out: &mut Profile) {
        let (gx, gz) = ((x / STEP) as usize, (z / STEP) as usize);
        let (fx, fz) = ((x % STEP) as f32 / STEP as f32, (z % STEP) as f32 / STEP as f32);
        let plane = SIDE * SIDE;
        let corner = gz * SIDE + gx;
        for layer in 0..self.layers {
            let at = |dx: usize, dz: usize| self.values[layer * plane + corner + dz * SIDE + dx];
            out.values[layer] = lerp(lerp(at(0, 0), at(1, 0), fx), lerp(at(0, 1), at(1, 1), fx), fz);
        }
        out.first = self.first;
        out.layers = self.layers;
        out.step_y = self.step_y;
    }
}

/// The values of a lattice above one block column, interpolated in y on request.
pub struct Profile {
    values: [f32; MAX_LAYERS],
    first: i32,
    layers: usize,
    step_y: i32,
}

impl Default for Profile {
    fn default() -> Self {
        Self { values: [0.0; MAX_LAYERS], first: 0, layers: 0, step_y: STEP }
    }
}

impl Profile {
    /// The value at height `y`, which must lie in the range the lattice was built for.
    #[inline]
    pub fn at(&self, y: i32) -> f32 {
        let layer = (y.div_euclid(self.step_y) - self.first) as usize;
        debug_assert!(layer + 1 < self.layers, "height {y} outside the lattice");
        let t = y.rem_euclid(self.step_y) as f32 / self.step_y as f32;
        lerp(self.values[layer], self.values[layer + 1], t)
    }
}

/// 2D values on a lattice over one column, bilinearly interpolated.
pub struct Grid {
    values: [f32; SIDE * SIDE],
}

impl Grid {
    /// Samples `f` at every lattice point over the column at `origin`.
    pub fn new(origin: IVec3, mut f: impl FnMut(i32, i32) -> f32) -> Self {
        Self { values: std::array::from_fn(|i| f(origin.x + (i % SIDE) as i32 * STEP, origin.z + (i / SIDE) as i32 * STEP)) }
    }

    pub fn noise(noise: &FastNoiseLite, origin: IVec3) -> Self {
        Self::new(origin, |x, z| noise.get_noise_2d(x as f32, z as f32))
    }

    /// The largest value anywhere over the column: interpolation never exceeds it.
    pub fn max(&self) -> f32 {
        self.values.iter().copied().fold(f32::MIN, f32::max)
    }

    /// The value at (x, z), relative to the column origin.
    #[inline]
    pub fn at(&self, x: i32, z: i32) -> f32 {
        let (gx, gz) = ((x / STEP) as usize, (z / STEP) as usize);
        let (fx, fz) = ((x % STEP) as f32 / STEP as f32, (z % STEP) as f32 / STEP as f32);
        let at = |dx: usize, dz: usize| self.values[(gz + dz) * SIDE + gx + dx];
        lerp(lerp(at(0, 0), at(1, 0), fx), lerp(at(0, 1), at(1, 1), fx), fz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_linear_fields_exactly() {
        let origin = IVec3::new(64, 0, -32);
        let field = |p: IVec3| p.x as f32 * 0.5 - p.y as f32 * 0.25 + p.z as f32;
        for step_y in [4, 8] {
            let lattice = Lattice::new(origin, 13, 200, step_y, field);
            let mut profile = Profile::default();
            for (x, z) in [(0, 0), (5, 31), (31, 17)] {
                lattice.profile(x, z, &mut profile);
                for y in [13, 64, 99, 200] {
                    assert!((profile.at(y) - field(origin + IVec3::new(x, y, z))).abs() < 1e-3);
                }
            }
        }
        let grid = Grid::new(origin, |x, z| (x - 3 * z) as f32);
        assert!((grid.at(7, 30) - (71 - 3 * -2) as f32).abs() < 1e-3);
    }
}
