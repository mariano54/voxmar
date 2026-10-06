//! The aether shimmer: how a supernatural creature's ordinary voice is made slightly
//! otherworldly, once, when its clips load. The voice stays recognisable (a moonhare still
//! sounds like a hare) but wavers as if heard through water, rings faintly like a wet
//! glass rim, and leaves a bright, airy echo behind, in keeping with the world's quiet
//! fantasy: never a monster-movie warble.

use std::f32::consts::TAU;

use crate::sfx::dsp::{OnePole, Svf};

/// Seconds of echo added after the voice at full shimmer.
const TAIL: f32 = 1.1;

/// `samples` (mono, at `rate`) with `amount` (0..1) of shimmer: a slow, detuned chorus,
/// glassy resonances rung by the voice, and a bright echo. `seed` picks the resonances, so
/// each species rings at its own chord.
pub fn shimmer(samples: &[f32], rate: u32, amount: f32, seed: u64) -> Vec<f32> {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 || samples.is_empty() {
        return samples.to_vec();
    }
    let sr = rate as f32;
    let len = samples.len() + (TAIL * amount * sr) as usize;
    let input = |i: usize| samples.get(i).copied().unwrap_or(0.0);
    let peak = samples.iter().fold(1e-6f32, |p, s| p.max(s.abs()));

    // Chorus: two copies read through slowly swaying delays, one a touch sharp of the
    // other, so the voice beats against itself.
    let chorus = |i: usize, rate_hz: f32, phase: f32, depth: f32| {
        let t = i as f32 / sr;
        let delay = (0.014 + depth * (TAU * rate_hz * t + phase).sin()) * sr;
        let at = i as f32 - delay;
        if at < 0.0 {
            return 0.0;
        }
        let (whole, part) = (at as usize, at.fract());
        input(whole) * (1.0 - part) + input(whole + 1) * part
    };

    // Resonators a fifth and an octave apart, tuned from the seed within a bright band.
    let root = 880.0 * 2f32.powf((crate::sfx::mix(seed) >> 40) as f32 / (1u64 << 24) as f32);
    let mut bells = [Svf::new(root, 40.0), Svf::new(root * 1.5, 45.0), Svf::new(root * 2.0, 50.0)];
    let mut bright = OnePole::new(350.0);

    // Schroeder's reverb: four combs into two allpasses, fed only the voice's upper half
    // so the echo is airy rather than boomy.
    let combs_ms = [29.7, 37.1, 41.1, 43.7];
    let mut combs: Vec<(Vec<f32>, usize)> = combs_ms.iter().map(|ms| (vec![0.0; (ms * 0.001 * sr * 1.6) as usize], 0)).collect();
    let mut allpasses: Vec<(Vec<f32>, usize)> = [5.0, 1.7].iter().map(|ms| (vec![0.0; (ms * 0.001 * sr) as usize], 0)).collect();
    let feedback = 0.72 + 0.12 * amount;

    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let dry = input(i);
        let wet = 0.5 * (chorus(i, 0.37, 0.0, 0.004) + chorus(i, 0.53, 2.1, 0.005));
        let voice = dry * (1.0 - 0.4 * amount) + wet * 0.55 * amount;
        let high = bright.high(dry);
        let ring: f32 = bells.iter_mut().map(|b| b.band(high)).sum::<f32>() * 0.6;
        let mut echo = 0.0;
        for (line, at) in &mut combs {
            let delayed = line[*at];
            line[*at] = high + delayed * feedback;
            *at = (*at + 1) % line.len();
            echo += delayed;
        }
        echo *= 0.25;
        for (line, at) in &mut allpasses {
            let delayed = line[*at];
            let x = echo + 0.5 * delayed;
            line[*at] = x;
            *at = (*at + 1) % line.len();
            echo = delayed - 0.5 * x;
        }
        out.push(voice + amount * (0.35 * ring + 0.45 * echo));
    }
    // As loud at its peak as the voice was, the echo's end faded so it never clicks.
    let top = out.iter().fold(1e-6f32, |p, s| p.max(s.abs()));
    let fade = ((0.25 * sr) as usize).min(len);
    for (i, s) in out.iter_mut().enumerate() {
        *s *= peak / top * ((len - i) as f32 / fade as f32).min(1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_keeps_the_voice_and_adds_an_echo() {
        let rate = crate::sfx::RATE;
        let voice: Vec<f32> = (0..rate / 2).map(|i| 0.5 * (TAU * 300.0 * i as f32 / rate as f32).sin() * (i as f32 / 2000.0).min(1.0)).collect();
        let none = shimmer(&voice, rate, 0.0, 1);
        assert_eq!(none, voice);
        let full = shimmer(&voice, rate, 1.0, 1);
        assert!(full.len() > voice.len() + rate as usize / 2, "an echo follows");
        assert!(full.iter().all(|s| s.is_finite() && s.abs() <= 0.5 + 1e-3), "as loud as the voice at most");
        // The echo is still sounding a little after the voice stops, and fades away.
        let after = &full[voice.len() + rate as usize / 10..voice.len() + rate as usize / 5];
        assert!(after.iter().any(|s| s.abs() > 0.005));
        assert!(full.last().unwrap().abs() < 1e-3);
    }
}
