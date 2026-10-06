//! How hard a blow lands for how long the striker waited: Minecraft's swing charge without
//! its knockback, tuned so timing matters a little and spamming is never worth much.
//!
//! A full-strength blow takes [`FULL_CHARGE`] seconds since the last; blows come no faster
//! than [`QUICKEST`]. In between, strength rises in a straight line from [`SPAM_STRENGTH`]
//! to 1. Damage per second is then `strength(t) / t`, which is highest for well-timed blows
//! and 90% of that at the quickest (Mariano, 2026-10-06: spam-clicking about 10% worse);
//! waiting longer than a full charge gains nothing.

/// Seconds since the last blow for one at full strength.
pub const FULL_CHARGE: f32 = 0.6;
/// The least seconds between blows: quicker ones are not taken.
pub const QUICKEST: f32 = 0.3;
/// Strength of a blow at the quickest.
pub const SPAM_STRENGTH: f32 = 0.45;

/// The share of its full damage a blow `since` seconds after the last one deals (`None`
/// for the first blow in a while: full).
pub fn strength(since: Option<f32>) -> f32 {
    let Some(since) = since else { return 1.0 };
    let t = ((since - QUICKEST) / (FULL_CHARGE - QUICKEST)).clamp(0.0, 1.0);
    SPAM_STRENGTH + (1.0 - SPAM_STRENGTH) * t
}

/// Whole damage from `full` scaled by `strength`, the fraction rounded up with chance equal
/// to it (drawn as `roll` in 0..1), so the average is exact even for small blows.
pub fn scaled(full: u16, strength: f32, roll: f32) -> u16 {
    let exact = full as f32 * strength.clamp(0.0, 1.0);
    exact.floor() as u16 + u16::from(roll < exact.fract())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_blows_beat_spam_by_a_tenth_and_waiting_longer_gains_nothing() {
        let dps = |t: f32| strength(Some(t)) / t;
        let timed = dps(FULL_CHARGE);
        assert!((dps(QUICKEST) / timed - 0.9).abs() < 1e-4);
        for t in [0.35, 0.4, 0.5, 0.55] {
            assert!(dps(t) < timed && dps(t) > 0.9 * timed - 1e-4, "{t}");
        }
        assert!(dps(1.2) < timed, "waiting past a full charge only loses damage per second");
        assert_eq!(strength(None), 1.0);
    }

    #[test]
    fn scaled_damage_averages_exactly() {
        let mean: f32 = (0..1000).map(|i| scaled(3, 0.45, i as f32 / 1000.0) as f32).sum::<f32>() / 1000.0;
        assert!((mean - 1.35).abs() < 0.01, "{mean}");
        assert_eq!(scaled(4, 1.0, 0.99), 4);
    }
}
