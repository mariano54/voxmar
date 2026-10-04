//! Where generation spends its time, stage by stage, for the benchmarks.
//!
//! Generation marks each stage with [`enter`], whose guard ends the stage when dropped.
//! Stages nest: entering one pauses the one around it, so every moment counts towards
//! exactly one stage, and a stage that calls into another (decoration asking for the
//! ground beyond the column, say) is not charged for it. Only test builds keep the clock,
//! and only on a thread inside [`measure`]; in every other build `enter` is empty and
//! compiles away.

/// A stage of column generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Stage {
    /// Anything not inside another stage.
    Other,
    /// The regional climate lattice and each block column's climate, colours and fields.
    Climate,
    /// Biome picks and borders.
    Biomes,
    /// The land with its rivers and lakes carved into each block column, and sealing.
    Rivers,
    /// Building a region's river network (routing, lakes, flow), on a cache miss.
    RiverRegions,
    /// Trails stamped into the ground.
    Trails,
    /// Planning a region's trails, on a cache miss.
    TrailPlans,
    /// Surface rules: the top blocks of each block column.
    Surface,
    /// Cave carvers, water tables and sealing.
    Caves,
    /// Cave biomes: picks, linings, decorations, geodes and fossils.
    CaveBiomes,
    /// Filling the ground with stone, strata and deepslate.
    Fill,
    /// Terrain shapes built in 3D: arches, leaning spires and the notches of sea cliffs.
    Shapes,
    Ores,
    Trees,
    /// Ground decoration, trail markers, small plants and the snow blanket.
    Decoration,
    /// Packing the blocks into palette-compressed chunks.
    Pack,
}

impl Stage {
    #[cfg(test)]
    pub const ALL: [Stage; 16] = [
        Stage::Other,
        Stage::Climate,
        Stage::Biomes,
        Stage::Rivers,
        Stage::RiverRegions,
        Stage::Trails,
        Stage::TrailPlans,
        Stage::Surface,
        Stage::Caves,
        Stage::CaveBiomes,
        Stage::Fill,
        Stage::Shapes,
        Stage::Ores,
        Stage::Trees,
        Stage::Decoration,
        Stage::Pack,
    ];
}

/// Starts `stage`, until the guard is dropped.
#[cfg(not(test))]
#[inline(always)]
pub(crate) fn enter(_stage: Stage) -> Guard {
    Guard
}

/// Ends its stage when dropped.
#[cfg(not(test))]
pub(crate) struct Guard;

/// Empty, but there, so that ending a stage early with `drop` reads the same in every build.
#[cfg(not(test))]
impl Drop for Guard {
    #[inline(always)]
    fn drop(&mut self) {}
}

#[cfg(test)]
pub(crate) use clock::{Times, enter, measure};

#[cfg(test)]
mod clock {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::Stage;

    /// A tick counter far cheaper to read than `Instant` (which costs about as much as a
    /// short stage), and the length of its ticks in nanoseconds: the CPU's own counter.
    #[cfg(target_arch = "aarch64")]
    fn ticks() -> u64 {
        let ticks: u64;
        // SAFETY: reading the virtual counter, which user code may, has no side effects.
        unsafe { std::arch::asm!("mrs {}, cntvct_el0", out(reg) ticks, options(nomem, nostack)) };
        ticks
    }

    #[cfg(target_arch = "aarch64")]
    fn tick_nanos() -> f64 {
        let hertz: u64;
        // SAFETY: as above, for the counter's frequency.
        unsafe { std::arch::asm!("mrs {}, cntfrq_el0", out(reg) hertz, options(nomem, nostack)) };
        1e9 / hertz as f64
    }

    #[cfg(not(target_arch = "aarch64"))]
    fn ticks() -> u64 {
        static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
        START.elapsed().as_nanos() as u64
    }

    #[cfg(not(target_arch = "aarch64"))]
    fn tick_nanos() -> f64 {
        1.0
    }

    /// Time spent in each stage, in `Stage::ALL` order.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Times(pub [Duration; Stage::ALL.len()]);

    impl Times {
        pub fn get(&self, stage: Stage) -> Duration {
            self.0[stage as usize]
        }

        pub fn total(&self) -> Duration {
            self.0.iter().sum()
        }
    }

    impl std::ops::AddAssign for Times {
        fn add_assign(&mut self, other: Self) {
            for (a, b) in self.0.iter_mut().zip(other.0) {
                *a += b;
            }
        }
    }

    struct Clock {
        /// Ticks spent in each stage.
        ticks: [u64; Stage::ALL.len()],
        /// The stages entered and not yet left, innermost last.
        stack: Vec<Stage>,
        since: u64,
    }

    impl Clock {
        /// Charges the time since the last change to the innermost stage.
        fn charge(&mut self, now: u64) {
            let current = *self.stack.last().unwrap_or(&Stage::Other);
            self.ticks[current as usize] += now - self.since;
            self.since = now;
        }
    }

    thread_local! {
        static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    }

    /// Runs `f` with this thread's stage clock running, and returns the time spent in each
    /// stage along with its result.
    pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Times) {
        CLOCK.with_borrow_mut(|clock| *clock = Some(Clock { ticks: [0; Stage::ALL.len()], stack: Vec::new(), since: ticks() }));
        let result = f();
        let spent = CLOCK.with_borrow_mut(|clock| {
            let mut clock = clock.take().expect("started above");
            clock.charge(ticks());
            clock.ticks
        });
        let scale = tick_nanos();
        (result, Times(spent.map(|t| Duration::from_nanos((t as f64 * scale) as u64))))
    }

    pub struct Guard(bool);

    pub fn enter(stage: Stage) -> Guard {
        Guard(CLOCK.with_borrow_mut(|clock| {
            let Some(clock) = clock else { return false };
            clock.charge(ticks());
            clock.stack.push(stage);
            true
        }))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if self.0 {
                CLOCK.with_borrow_mut(|clock| {
                    if let Some(clock) = clock {
                        clock.charge(ticks());
                        clock.stack.pop();
                    }
                });
            }
        }
    }
}
