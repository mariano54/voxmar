//! The sky's calendar, one for the server and every client: the moon's phases and the
//! year the stars turn over. Everything takes `days`, the days elapsed since the world
//! began (its clock's ticks over `TICKS_PER_DAY`, fractions included), so the same moment
//! gives the same moon everywhere: the client draws it and lights the night by it, and the
//! server can let creatures, drops and plants follow it.

/// Days from one full moon to the next.
pub const MONTH_DAYS: f64 = 10.0;
/// Days for the stars to turn once against the sun, so that each season has its own
/// constellations at midnight.
pub const YEAR_DAYS: f64 = 100.0;
/// The days the sky's own clock runs before it wraps; the month and year divide it, and so
/// does every period the sky draws, so the wrap never shows.
pub const SKY_DAYS: f64 = 1000.0;

/// Where in its month the moon is on `days`, 0..1: 0 full, 0.25 waning half, 0.5 new,
/// 0.75 waxing half. It is full on day 0.
pub fn moon_phase(days: f64) -> f32 {
    (days / MONTH_DAYS).rem_euclid(1.0) as f32
}

/// How much of the moon's face is lit on `days`: 1 full, 0 new.
pub fn moon_lit(days: f64) -> f32 {
    0.5 + 0.5 * (moon_phase(days) * std::f32::consts::TAU).cos()
}

/// Where in its year the sky is on `days`, 0..1.
pub fn year_fraction(days: f64) -> f32 {
    (days / YEAR_DAYS).rem_euclid(1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_moon_wanes_to_new_and_waxes_back() {
        assert_eq!(moon_lit(0.0), 1.0);
        assert!(moon_lit(MONTH_DAYS / 2.0) < 1e-6);
        assert!((moon_lit(MONTH_DAYS / 4.0) - 0.5).abs() < 1e-6);
        assert!((moon_lit(3.0 * MONTH_DAYS) - 1.0).abs() < 1e-6);
        assert!(moon_lit(1.0) < moon_lit(0.5) && moon_lit(9.0) > moon_lit(8.0));
        assert!((SKY_DAYS / MONTH_DAYS).fract() == 0.0 && (SKY_DAYS / YEAR_DAYS).fract() == 0.0);
    }
}
