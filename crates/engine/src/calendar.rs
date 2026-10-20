//! The sky's calendar, one for the server and every client: the sun's path through the
//! day, the moon's phases and the year the stars turn over. Everything takes `days`, the days elapsed since the world
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

/// The sun's arc is lifted above the horizon by this much of its radius, so that it is up
/// for two thirds of the day and down for one (Mariano: day two thirds, night one third),
/// while it still moves at one steady pace, so dawn and dusk stay gradual. The arc
/// starts at sunrise, at a fraction of 0, as it always has.
const LIFT: f64 = 0.5;
/// How far the arc leans towards the horizon's side, against its radius.
const TILT: f64 = 0.25;
/// The share of the day the sun is up.
pub const DAYLIGHT_SHARE: f64 = 2.0 / 3.0;

/// The sine of `x`, by Bhaskara's approximation: plain arithmetic, so every machine
/// works out the same sun to the last bit.
fn sine(x: f64) -> f64 {
    let pi = std::f64::consts::PI;
    let x = x.rem_euclid(2.0 * pi);
    let (x, sign) = if x <= pi { (x, 1.0) } else { (x - pi, -1.0) };
    sign * 16.0 * x * (pi - x) / (5.0 * pi * pi - 4.0 * x * (pi - x))
}

/// Towards the sun at `day_fraction` (0..1 from sunrise), unnormalised x (along its arc
/// across), y (up) and z (the lean): the sun rises at 0, is highest a third of the way
/// through the day, sets two thirds of the way, and is lowest at five sixths.
pub fn sun_vector(day_fraction: f64) -> [f64; 3] {
    // Lifted by half its radius, the arc meets the horizon 30° below its middle.
    let angle = day_fraction * std::f64::consts::TAU - std::f64::consts::FRAC_PI_6;
    [sine(angle + std::f64::consts::FRAC_PI_2), sine(angle) + LIFT, TILT]
}

/// The sine of the sun's height above the horizon at `day_fraction`: 0 at sunrise and
/// sunset, about 0.99 at noon, about -0.89 at midnight.
pub fn sun_height(day_fraction: f64) -> f64 {
    let [x, y, z] = sun_vector(day_fraction);
    y / (x * x + y * y + z * z).sqrt()
}

/// Whether it is night at `day_fraction` (0..1 from sunrise): from sunset, two thirds of
/// the way through the day (`DAYLIGHT_SHARE`), up to sunrise. Sunset itself is night and
/// sunrise day, as `/time set` names them.
pub fn night(day_fraction: f64) -> bool {
    (DAYLIGHT_SHARE..1.0).contains(&day_fraction.rem_euclid(1.0))
}

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
    fn the_sun_is_up_two_thirds_of_the_day() {
        assert!(sun_height(0.0).abs() < 1e-3, "{}", sun_height(0.0));
        assert!(sun_height(DAYLIGHT_SHARE).abs() < 1e-3, "{}", sun_height(DAYLIGHT_SHARE));
        let up = (0..1000).filter(|i| sun_height(*i as f64 / 1000.0) > 0.0).count();
        assert!((up as f64 / 1000.0 - DAYLIGHT_SHARE).abs() < 0.005, "{up}");
        // Highest at a third of the day, lowest at five sixths.
        assert!(sun_height(1.0 / 3.0) > 0.98 && sun_height(5.0 / 6.0) < -0.85);
        // Night is the third of the day the sun is down, sunset to sunrise.
        assert!(!night(0.01) && !night(0.5) && !night(0.66) && night(0.67) && night(5.0 / 6.0) && night(0.99));
        // The edges as `/time set` names them: sunrise (0) is day, sunset (16000) night.
        assert!(!night(0.0) && night(16_000.0 / 24_000.0) && !night(15_999.0 / 24_000.0) && night(23_999.0 / 24_000.0) && !night(1.0));
    }

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
