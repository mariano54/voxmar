//! Fields: traits of space, stored coarsely per cell.
//!
//! A field's value is its generated baseline plus whatever has been emitted into it, and
//! only the emitted differences need saving. Fields are how loosely related systems
//! influence each other without naming each other: a process reads temperature without
//! knowing whether climate, a furnace or a mod put the heat there.

use rustc_hash::FxHashMap;

use crate::glam::IVec2;

/// Blocks per field cell along x and z.
pub const FIELD_CELL: i32 = 16;

type Baseline = Box<dyn Fn(i32, i32) -> f32 + Send + Sync>;

pub struct Field {
    name: String,
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
        Self { name: name.into(), baseline: Box::new(baseline), cache: FxHashMap::default(), deltas: FxHashMap::default() }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn cell(x: i32, z: i32) -> IVec2 {
        IVec2::new(x.div_euclid(FIELD_CELL), z.div_euclid(FIELD_CELL))
    }

    pub fn sample(&mut self, x: i32, z: i32) -> f32 {
        let cell = Self::cell(x, z);
        let base = *self.cache.entry(cell).or_insert_with(|| {
            let center = cell * FIELD_CELL + FIELD_CELL / 2;
            (self.baseline)(center.x, center.y)
        });
        base + self.deltas.get(&cell).copied().unwrap_or(0.0)
    }

    /// Adds `amount` to the cell containing (x, z).
    pub fn emit(&mut self, x: i32, z: i32, amount: f32) {
        *self.deltas.entry(Self::cell(x, z)).or_default() += amount;
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
}
