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
    /// `step_y` is `1 << shift_y`.
    shift_y: u32,
}

impl Lattice {
    /// Samples `f` at every lattice point of the column at `origin` whose layer is needed to
    /// interpolate heights `low..=high`, with layers `step_y` blocks apart (a power of two,
    /// so that interpolating in y needs no division).
    pub fn new(origin: IVec3, low: i32, high: i32, step_y: i32, f: impl FnMut(IVec3) -> f32) -> Self {
        let mut lattice = Self::empty(low, high, step_y);
        lattice.values = lattice.points(origin).map(f).collect();
        lattice
    }

    /// A noise field, with heights multiplied by `scale_y` (above 1 flattens its shapes),
    /// sampled many points at a time.
    pub fn noise(noise: &FastNoiseLite, origin: IVec3, low: i32, high: i32, step_y: i32, scale_y: f32) -> Self {
        let mut lattice = Self::empty(low, high, step_y);
        let points: Vec<[f32; 3]> = lattice.points(origin).map(|p| [p.x as f32, p.y as f32 * scale_y, p.z as f32]).collect();
        lattice.values = vec![0.0; points.len()];
        crate::noise::noise_3d(noise, &points, &mut lattice.values);
        lattice
    }

    /// A lattice with no values yet, for heights `low..=high` with layers `step_y` blocks
    /// apart (a power of two, so that interpolating in y needs no division).
    fn empty(low: i32, high: i32, step_y: i32) -> Self {
        assert!(step_y > 0 && step_y.count_ones() == 1, "lattice layers {step_y} apart");
        let first = low.div_euclid(step_y);
        let layers = (high.div_euclid(step_y) - first + 2).max(2) as usize;
        debug_assert!(layers <= MAX_LAYERS);
        Self { values: Vec::new(), first, layers, shift_y: step_y.trailing_zeros() }
    }

    /// The lattice's points, in the order of `values`: layer by layer, rows along x.
    fn points(&self, origin: IVec3) -> impl Iterator<Item = IVec3> + use<> {
        let (first, step_y) = (self.first, 1 << self.shift_y);
        (0..self.layers as i32).flat_map(move |layer| {
            (0..SIDE as i32).flat_map(move |lz| (0..SIDE as i32).map(move |lx| origin + IVec3::new(lx * STEP, (first + layer) * step_y, lz * STEP)))
        })
    }

    /// The values above the block column at (x, z), relative to the column origin.
    pub fn profile(&self, x: i32, z: i32, out: &mut Profile) {
        self.profile_to(x, z, i32::MAX, out);
    }

    /// The values above the block column at (x, z) like `profile`, for heights up to `high`
    /// only: the layers above it are left out. The profile may then be read at heights up
    /// to `high` and no higher: past it, `Profile::at` and `Profile::fill` read layers this
    /// call did not fill, which only debug builds check, and release builds return what an
    /// earlier profile left there.
    pub fn profile_to(&self, x: i32, z: i32, high: i32, out: &mut Profile) {
        let (gx, gz) = ((x / STEP) as usize, (z / STEP) as usize);
        let (fx, fz) = ((x % STEP) as f32 / STEP as f32, (z % STEP) as f32 / STEP as f32);
        let plane = SIDE * SIDE;
        let corner = gz * SIDE + gx;
        let layers = ((high >> self.shift_y) - self.first + 2).clamp(0, self.layers as i32) as usize;
        for layer in 0..layers {
            let at = |dx: usize, dz: usize| self.values[layer * plane + corner + dz * SIDE + dx];
            out.values[layer] = lerp(lerp(at(0, 0), at(1, 0), fx), lerp(at(0, 1), at(1, 1), fx), fz);
        }
        out.first = self.first;
        out.layers = layers;
        out.shift_y = self.shift_y;
    }
}

/// The values of a lattice above one block column, interpolated in y on request.
pub struct Profile {
    values: [f32; MAX_LAYERS],
    first: i32,
    layers: usize,
    shift_y: u32,
}

impl Default for Profile {
    fn default() -> Self {
        Self { values: [0.0; MAX_LAYERS], first: 0, layers: 0, shift_y: STEP.trailing_zeros() }
    }
}

impl Profile {
    /// The value at height `y`, which must lie in the range the lattice was built for.
    #[inline]
    pub fn at(&self, y: i32) -> f32 {
        let layer = ((y >> self.shift_y) - self.first) as usize;
        debug_assert!(layer + 1 < self.layers, "height {y} outside the lattice");
        // The same as dividing by the step, which is a power of two.
        let t = (y & ((1 << self.shift_y) - 1)) as f32 * (1.0 / (1 << self.shift_y) as f32);
        lerp(self.values[layer], self.values[layer + 1], t)
    }

    /// The values at heights `from..from + out.len()`, into `out`: the same as `at`, a layer
    /// at a time.
    pub fn fill(&self, from: i32, out: &mut [f32]) {
        let (step, scale) = (1 << self.shift_y, 1.0 / (1 << self.shift_y) as f32);
        let end = from + out.len() as i32;
        let mut y = from;
        while y < end {
            let layer = ((y >> self.shift_y) - self.first) as usize;
            debug_assert!(layer + 1 < self.layers, "height {y} outside the lattice");
            let (a, b) = (self.values[layer], self.values[layer + 1]);
            let next = (((y >> self.shift_y) + 1) << self.shift_y).min(end);
            for (v, y) in out[(y - from) as usize..(next - from) as usize].iter_mut().zip(y..) {
                *v = lerp(a, b, (y & (step - 1)) as f32 * scale);
            }
            y = next;
        }
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
                let mut filled = vec![0.0; 150];
                profile.fill(37, &mut filled);
                assert!(filled.iter().zip(37..).all(|(&v, y)| v == profile.at(y)));
            }
        }
        let grid = Grid::new(origin, |x, z| (x - 3 * z) as f32);
        assert!((grid.at(7, 30) - (71 - 3 * -2) as f32).abs() < 1e-3);
    }
}
