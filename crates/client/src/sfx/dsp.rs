//! Small building blocks for the synthesized sounds, shared by the sound effects and the
//! ambience: filters, pink noise, slow wanders, and water's bubbles and nature's taps.

use std::f32::consts::{PI, TAU};

use super::{Noise, RATE};

const SR: f32 = RATE as f32;

/// Chamberlin state-variable filter: low-, band- and high-pass at once.
pub(crate) struct Svf {
    f: f32,
    damping: f32,
    low: f32,
    band: f32,
}

impl Svf {
    pub(crate) fn new(center: f32, q: f32) -> Self {
        let mut svf = Self { f: 0.0, damping: 1.0 / q, low: 0.0, band: 0.0 };
        svf.tune(center);
        svf
    }

    /// Kept well below the frequencies where this filter turns unstable.
    pub(crate) fn tune(&mut self, center: f32) {
        self.f = 2.0 * (PI * center.clamp(10.0, 7000.0) / SR).sin();
    }

    pub(crate) fn low(&mut self, x: f32) -> f32 {
        self.run(x).0
    }

    pub(crate) fn band(&mut self, x: f32) -> f32 {
        self.run(x).1
    }

    pub(crate) fn high(&mut self, x: f32) -> f32 {
        self.run(x).2
    }

    fn run(&mut self, x: f32) -> (f32, f32, f32) {
        self.low += self.f * self.band;
        let high = x - self.low - self.damping * self.band;
        self.band += self.f * high;
        (self.low, self.band, high)
    }
}

/// One-pole low-pass (and the high-pass left over), for gentle slopes.
pub(crate) struct OnePole {
    a: f32,
    y: f32,
}

impl OnePole {
    pub(crate) fn new(cutoff: f32) -> Self {
        Self { a: 1.0 - (-TAU * cutoff / SR).exp(), y: 0.0 }
    }

    pub(crate) fn low(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }

    pub(crate) fn high(&mut self, x: f32) -> f32 {
        x - self.low(x)
    }
}

/// Pink noise from white (Paul Kellet's economical filter): equal energy per octave, the
/// spectrum of rain, wind and running water rather than white noise's hiss.
#[derive(Default)]
pub(crate) struct Pink([f32; 3]);

impl Pink {
    pub(crate) fn next(&mut self, white: f32) -> f32 {
        let b = &mut self.0;
        b[0] = 0.99765 * b[0] + white * 0.099_046;
        b[1] = 0.963 * b[1] + white * 0.296_516_4;
        b[2] = 0.57 * b[2] + white * 1.052_691_3;
        (b[0] + b[1] + b[2] + white * 0.1848) * 0.2
    }
}

/// A slow, smooth wander in -1..1: a few sines of unrelated frequencies around `rate` Hz.
pub(crate) struct Drift([(f32, f32); 3]);

impl Drift {
    pub(crate) fn new(noise: &mut Noise, rate: f32) -> Self {
        Self([1.0, 2.13, 3.71].map(|k| (rate * k * (0.8 + 0.4 * noise.unit()), TAU * noise.unit())))
    }

    pub(crate) fn at(&self, t: f32) -> f32 {
        self.0.iter().zip([0.55, 0.3, 0.15]).map(|(&(f, phase), w)| w * (TAU * f * t + phase).sin()).sum()
    }

    /// 0..1, eased.
    pub(crate) fn unit(&self, t: f32) -> f32 {
        let x = 0.5 + 0.5 * self.at(t);
        x * x * (3.0 - 2.0 * x)
    }
}

/// Adds one air bubble ringing in water at `start` seconds (after Minnaert's bubble
/// resonance and Farnell's *Designing Sound*): a tone at the bubble's resonance that dies within about a dozen cycles and rises in pitch by up to `rise`
/// (a fraction) as the bubble nears the surface. Small bubbles are high and brief; a
/// brook is hundreds of them a second. Kept short and only slightly rising: long,
/// sweeping bubbles are the bloop of a cartoon.
pub(crate) fn bubble(out: &mut [f32], start: f32, frequency: f32, gain: f32, rise: f32) {
    let ring = 4.0 / frequency;
    let first = (start * SR) as usize;
    let (fade, fade_onset) = ((-1.0 / (ring * SR)).exp(), (-1.0 / (0.0004 * SR)).exp());
    let (mut decay, mut onset, mut phase) = (1.0f32, 1.0f32, 0.0f32);
    for j in 0..(ring * 6.0 * SR) as usize {
        let Some(sample) = out.get_mut(first + j) else { break };
        phase = (phase + frequency * (1.0 + rise * (1.0 - decay)) / SR).fract();
        // A softer onset avoids a click.
        *sample += gain * decay * (1.0 - onset) * (TAU * phase).sin();
        decay *= fade;
        onset *= fade_onset;
    }
}

/// Adds a tap of noise at `start` seconds: something small striking something (a drop on
/// a leaf, spray landing), `length` seconds to die away, coloured by a band-pass at
/// `center` Hz. No ringing tone, so a thousand of them hiss rather than buzz.
pub(crate) fn tap(out: &mut [f32], noise: &mut Noise, start: f32, length: f32, center: f32, gain: f32) {
    let first = (start * SR) as usize;
    let mut band = Svf::new(center, 0.8);
    let (fade, mut level) = ((-1.0 / (length * SR)).exp(), gain);
    for j in 0..(length * 4.0 * SR) as usize {
        let Some(sample) = out.get_mut(first + j) else { break };
        *sample += level * band.band(noise.next());
        level *= fade;
    }
}

/// Seconds to the next event of a random (Poisson) stream with `rate` events a second.
pub(crate) fn gap(noise: &mut Noise, rate: f32) -> f32 {
    -(1.0 - noise.unit()).max(1e-6).ln() / rate.max(1e-6)
}

/// Samples in `t` seconds.
pub(crate) fn seconds(t: f32) -> usize {
    (t.max(0.0) * SR) as usize
}

/// A uniform number between `low` and `high`.
pub(crate) fn between(noise: &mut Noise, low: f32, high: f32) -> f32 {
    low + (high - low) * noise.unit()
}

/// Scales `samples` to `rms`, unless that would push a peak past `peak`, and fades their
/// ends so they never click.
pub(crate) fn normalise(samples: &mut [f32], rms: f32, peak: f32) {
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
    let top = samples.iter().fold(1e-6f32, |p, s| p.max(s.abs()));
    let scale = (rms / power.sqrt().max(1e-6)).min(peak / top);
    let (fade_in, fade_out) = (seconds(0.002).max(1), seconds(0.02).max(1));
    let len = samples.len();
    for (i, sample) in samples.iter_mut().enumerate() {
        *sample *= scale * (i as f32 / fade_in as f32).min(1.0) * ((len - i) as f32 / fade_out as f32).min(1.0);
    }
}
