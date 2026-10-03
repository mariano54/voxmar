//! Fields: traits of space, stored coarsely per cell.
//!
//! A field's value is its generated baseline plus whatever has been emitted into it, and
//! only the emitted differences need saving. Fields are how loosely related systems
//! influence each other without naming each other: a process reads temperature without
//! knowing whether climate, a furnace or a mod put the heat there.

use rustc_hash::FxHashMap;

use crate::glam::IVec2;

/// Blocks per field cell along x and z, unless a field chooses another size.
pub const FIELD_CELL: i32 = 16;

type Baseline = Box<dyn Fn(i32, i32) -> f32 + Send + Sync>;

pub struct Field {
    name: String,
    /// Blocks per cell along x and z.
    cell_size: i32,
    baseline: Baseline,
    /// Baseline values already computed, per cell.
    cache: FxHashMap<IVec2, f32>,
    /// Emitted changes relative to the baseline, per cell.
    deltas: FxHashMap<IVec2, f32>,
}

impl Field {
    /// `baseline(x, z)` gives the generated value at a world position; it is sampled once
    /// per cell, at the cell centre.
    pub fn new(name: impl Into<String>, baseline: impl Fn(i32, i32) -> f32 + Send + Sync + 'static) -> Self {
        Self::with_cell_size(name, FIELD_CELL, baseline)
    }

    /// A field with cells `cell_size` blocks wide, for values that vary slowly over space
    /// such as the weather.
    pub fn with_cell_size(
        name: impl Into<String>,
        cell_size: i32,
        baseline: impl Fn(i32, i32) -> f32 + Send + Sync + 'static,
    ) -> Self {
        assert!(cell_size > 0, "field cells are at least one block wide");
        Self {
            name: name.into(),
            cell_size,
            baseline: Box::new(baseline),
            cache: FxHashMap::default(),
            deltas: FxHashMap::default(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn cell(&self, x: i32, z: i32) -> IVec2 {
        IVec2::new(x.div_euclid(self.cell_size), z.div_euclid(self.cell_size))
    }

    fn cell_baseline(&mut self, cell: IVec2) -> f32 {
        let center = cell * self.cell_size + self.cell_size / 2;
        *self.cache.entry(cell).or_insert_with(|| (self.baseline)(center.x, center.y))
    }

    /// The generated value at (x, z), without anything emitted into it.
    pub fn baseline(&mut self, x: i32, z: i32) -> f32 {
        self.cell_baseline(self.cell(x, z))
    }

    pub fn sample(&mut self, x: i32, z: i32) -> f32 {
        let cell = self.cell(x, z);
        self.cell_baseline(cell) + self.deltas.get(&cell).copied().unwrap_or(0.0)
    }

    /// Adds `amount` to the cell containing (x, z).
    pub fn emit(&mut self, x: i32, z: i32, amount: f32) {
        *self.deltas.entry(self.cell(x, z)).or_default() += amount;
    }

    /// Sets the cell containing (x, z) to `value`, for a process that evolves the field
    /// rather than adding to it.
    pub fn set(&mut self, x: i32, z: i32, value: f32) {
        let cell = self.cell(x, z);
        let delta = value - self.cell_baseline(cell);
        self.deltas.insert(cell, delta);
    }

    /// Returns the cell containing (x, z) to its baseline.
    pub fn reset(&mut self, x: i32, z: i32) {
        self.deltas.remove(&self.cell(x, z));
    }

    /// The emitted differences: the only part of a field that needs persisting.
    pub fn deltas(&self) -> impl Iterator<Item = (IVec2, f32)> + '_ {
        self.deltas.iter().map(|(&cell, &value)| (cell, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_plus_emissions() {
        let mut field = Field::new("test:heat", |x, _| x as f32);
        assert_eq!(field.sample(3, 0), 8.0);
        field.emit(5, 5, 2.0);
        assert_eq!(field.sample(0, 15), 10.0);
        assert_eq!(field.sample(-1, 0), -8.0);
        assert_eq!(field.deltas().count(), 1);
    }

    #[test]
    fn coarse_cells_set_and_reset() {
        let mut field = Field::with_cell_size("test:wet", 128, |_, z| z as f32);
        assert_eq!(field.sample(10, -1), -64.0);
        field.set(127, 0, 1.0);
        assert_eq!(field.sample(0, 127), 1.0);
        assert_eq!(field.baseline(0, 127), 64.0);
        assert_eq!(field.deltas().next(), Some((IVec2::ZERO, -63.0)));
        field.reset(5, 5);
        assert_eq!(field.sample(5, 5), 64.0);
        assert_eq!(field.deltas().count(), 0);
    }
}
