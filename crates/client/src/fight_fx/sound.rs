//! Fight sounds made from `combat_fx.ron`'s layers at startup: deterministic, so a hit
//! sounds the same on every machine, and nothing is shipped. A sound a package gives
//! recordings for plays those instead.
//!
//! What makes a blow satisfying is built into the layers rather than any one of them: a
//! contact click of a few milliseconds, a body's thump that falls in pitch as it dies (the
//! weight), a band of noise for the surface that was struck (the slap), saturated together
//! so they glue into one sound, with a short room tail so it sits in the world.

use std::f32::consts::{PI, TAU};

use crate::sfx::dsp::Svf;
use crate::sfx::{Noise, RATE, mix};

use super::style::{Layer, SoundDef};

const SR: f32 = RATE as f32;
/// Peak of every made sound; the playback gain sets its loudness.
const PEAK: f32 = 0.9;
/// Seconds of tail a room adds.
const ROOM_TAIL: f32 = 0.22;

/// The takes of a sound: `variants` renders of its layers, each nudged a little in pitch
/// and time by its seed.
pub fn render(def: &SoundDef, seed: u64) -> Vec<Vec<f32>> {
    (0..def.variants as u64).map(|v| take(def, mix(seed ^ v.wrapping_mul(0x9e37)))).collect()
}

/// One take.
pub fn take(def: &SoundDef, seed: u64) -> Vec<f32> {
    let mut noise = Noise(seed);
    // Each take a little higher or lower and quicker or slower: never the same twice.
    let pitch = 1.0 + (noise.unit() - 0.5) * 0.1;
    let pace = 1.0 + (noise.unit() - 0.5) * 0.14;
    let longest = def.layers.iter().map(|l| l.seconds() * pace).fold(0.0, f32::max);
    let tail = if def.room > 0.0 { ROOM_TAIL } else { 0.0 };
    let length = ((longest + tail + 0.01) * SR) as usize;
    let mut out = vec![0.0f32; length];
    for layer in &def.layers {
        add(&mut out, layer, pitch, pace, &mut noise);
    }
    if def.drive > 0.0 {
        let k = 1.0 + def.drive;
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
        for s in &mut out {
            *s = (*s / peak * k).tanh() / k.tanh();
        }
    }
    if def.room > 0.0 {
        room(&mut out, def.room);
    }
    // Ends at silence, levelled to the common peak.
    let fade = (0.004 * SR) as usize;
    let n = out.len();
    for (i, s) in out.iter_mut().enumerate().skip(n.saturating_sub(fade)) {
        *s *= (n - i) as f32 / fade as f32;
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 1e-6 {
        for s in &mut out {
            *s *= PEAK / peak;
        }
    }
    out
}

/// Decay to a thousandth (-60 dB) over `seconds`.
fn decay(t: f32, seconds: f32) -> f32 {
    (-6.9 * t / seconds.max(1e-4)).exp()
}

/// Adds a layer into `out`.
fn add(out: &mut [f32], layer: &Layer, pitch: f32, pace: f32, noise: &mut Noise) {
    let len = out.len();
    let samples = |seconds: f32| ((seconds * pace * SR) as usize).min(len);
    match *layer {
        Layer::Click { gain } => {
            // A few milliseconds of bright noise, the instant of contact.
            let mut high = Svf::new(2500.0, 0.7);
            for (i, s) in out.iter_mut().take(samples(0.006)).enumerate() {
                let t = i as f32 / SR;
                *s += high.high(noise.next()) * (-t / 0.0012).exp() * gain;
            }
        }
        Layer::Tone { from, to, seconds, gain } => {
            // A sine falling (or rising) from one pitch to the other early in its life, as a
            // struck body's does, dying away over `seconds`.
            let (from, to, seconds) = (from * pitch, to * pitch, seconds * pace);
            let mut phase = 0.0f32;
            for (i, s) in out.iter_mut().take(samples(seconds / pace)).enumerate() {
                let t = i as f32 / SR;
                let freq = to + (from - to) * (-t / (seconds * 0.22)).exp();
                phase = (phase + TAU * freq / SR) % TAU;
                let attack = (t / 0.0015).min(1.0);
                *s += phase.sin() * attack * decay(t, seconds) * gain;
            }
        }
        Layer::Noise { low, high, attack, seconds, gain } => {
            let (low, high, seconds, attack) = (low * pitch, high * pitch, seconds * pace, attack * pace);
            let mut band = band(low, high);
            for (i, s) in out.iter_mut().take(samples(seconds / pace)).enumerate() {
                let t = i as f32 / SR;
                let swell = if attack > 0.0 { (t / attack).min(1.0) } else { 1.0 };
                let after = (t - attack).max(0.0);
                *s += run_band(&mut band, noise.next()) * swell * decay(after, seconds - attack) * gain;
            }
        }
        Layer::Sweep { from, to, seconds, gain } => {
            // Air past a blade: noise through a band that rises and falls with the swing's
            // speed, swelling quickly and trailing off.
            let (from, to, seconds) = (from * pitch, to * pitch, seconds * pace);
            // Two band-passes in a row, so it is air moving rather than a hiss.
            let (mut first, mut second) = (Svf::new(from, 1.4), Svf::new(from, 1.4));
            let n = samples(seconds / pace);
            for (i, s) in out.iter_mut().take(n).enumerate() {
                let x = i as f32 / n.max(1) as f32;
                let speed = (PI * x.powf(0.6)).sin();
                let centre = from * (to / from).powf(speed);
                first.tune(centre);
                second.tune(centre);
                *s += second.band(first.band(noise.next())) * speed.powf(1.6) * gain * 2.4;
            }
        }
        Layer::Ring { freq, ref partials, seconds, gain } => {
            // Struck metal or wood: inharmonic partials, the higher dying sooner.
            let (freq, seconds) = (freq * pitch, seconds * pace);
            for (k, ratio) in partials.iter().enumerate() {
                let f = freq * ratio;
                let life = seconds / (1.0 + 0.7 * k as f32);
                let amp = gain / (1.0 + k as f32 * 0.8);
                let start = noise.unit() * TAU;
                for (i, s) in out.iter_mut().take(samples(life / pace)).enumerate() {
                    let t = i as f32 / SR;
                    *s += (start + TAU * f * t).sin() * decay(t, life) * amp * (t / 0.0008).min(1.0);
                }
            }
        }
        Layer::Crackle { low, high, grains, seconds, gain } => {
            // Sparse grains, crowded at the start: bone, bark, grit.
            let (low, high, seconds) = (low * pitch, high * pitch, seconds * pace);
            let mut grain = vec![0.0f32; out.len()];
            for _ in 0..grains {
                let at = ((noise.unit().powi(2) * seconds) * SR) as usize;
                let size = noise.unit() * 0.7 + 0.3;
                let width = ((0.0008 + noise.unit() * 0.0025) * SR) as usize;
                for j in 0..width {
                    if let Some(g) = grain.get_mut(at + j) {
                        *g += noise.next() * size * (1.0 - j as f32 / width as f32);
                    }
                }
            }
            let mut band = band(low, high);
            for (i, (s, g)) in out.iter_mut().zip(&grain).enumerate() {
                let t = i as f32 / SR;
                *s += run_band(&mut band, *g) * decay(t, seconds * 1.5) * gain * 1.4;
            }
        }
    }
}

/// A band from `low` to `high`: a high-pass and a low-pass in a row.
fn band(low: f32, high: f32) -> (Svf, Svf) {
    (Svf::new(low, 0.7), Svf::new(high, 0.7))
}

fn run_band(band: &mut (Svf, Svf), x: f32) -> f32 {
    let x = band.0.high(x);
    band.1.low(x)
}

/// A small room: three damped feedback echoes, mixed in at `wet`.
fn room(out: &mut [f32], wet: f32) {
    let dry = out.to_vec();
    let mut wet_sum = vec![0.0f32; out.len()];
    for (delay, feedback) in [(0.0233, 0.45), (0.0317, 0.4), (0.0411, 0.35)] {
        let d = (delay * SR) as usize;
        let mut line = vec![0.0f32; out.len()];
        let mut damp = 0.0f32;
        for i in 0..out.len() {
            let back = if i >= d { line[i - d] } else { 0.0 };
            damp += 0.45 * (back - damp);
            line[i] = dry[i] + damp * feedback;
            wet_sum[i] += back;
        }
    }
    for (s, w) in out.iter_mut().zip(wet_sum) {
        *s += w * wet / 3.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fight_fx::style::FightLook;

    fn look() -> FightLook {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/core/combat_fx.ron")).unwrap();
        FightLook::load(&[voxmar_engine::content::ContentFile { package: "core".into(), name: super::super::style::FILE.into(), source }]).unwrap()
    }

    /// The energy of `samples` in a band, by a crude DFT at a few frequencies.
    fn energy(samples: &[f32], low: f32, high: f32) -> f32 {
        let n = samples.len().min(8192);
        let mut sum = 0.0;
        let mut f = low;
        while f < high {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, s) in samples.iter().take(n).enumerate() {
                let a = TAU * f * i as f32 / SR;
                re += s * a.cos();
                im += s * a.sin();
            }
            sum += (re * re + im * im).sqrt();
            f *= 1.12;
        }
        sum
    }

    #[test]
    fn every_sound_is_made_and_levelled() {
        let look = look();
        for (name, def) in &look.sounds {
            let takes = render(def, 7);
            assert_eq!(takes.len(), def.variants as usize, "{name}");
            for take in &takes {
                let peak = take.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!((peak - PEAK).abs() < 1e-3, "{name} peaks at {peak}");
                assert!(take.iter().all(|s| s.is_finite()), "{name} has a sample that is not a number");
                // Short: a fight sound never lingers past a second.
                assert!(take.len() < SR as usize, "{name} lasts {} s", take.len() as f32 / SR);
                // It ends in silence, never on a click.
                assert!(take.last().unwrap().abs() < 0.02, "{name} ends at {}", take.last().unwrap());
            }
            // Takes differ.
            if takes.len() > 1 {
                assert_ne!(takes[0], takes[1], "{name}'s takes are the same");
            }
        }
    }

    /// Fights as they sound, then every fight sound in turn, as one WAV file to listen to:
    /// `VOXMAR_FIGHT_PREVIEW=path cargo test -p voxmar_client write_a_preview -- --ignored`.
    #[test]
    #[ignore]
    fn write_a_preview() {
        let Some(path) = std::env::var_os("VOXMAR_FIGHT_PREVIEW") else { return };
        let look = look();
        let mut all = Vec::new();
        let mut take_of = std::collections::BTreeMap::<&str, u64>::new();
        let mut put = |all: &mut Vec<f32>, at: f32, name: &'static str| {
            let def = &look.sounds[name];
            let n = take_of.entry(name).or_default();
            *n += 1;
            let samples = take(def, *n);
            let start = (at * SR) as usize;
            if all.len() < start + samples.len() {
                all.resize(start + samples.len(), 0.0);
            }
            for (i, s) in samples.iter().enumerate() {
                all[start + i] += s * def.gain;
            }
        };
        // A sword: three blows, the last two on the beat, a flourish, and the kill.
        let scenes: [&[(f32, &str)]; 7] = [
            &[(0.0, "swing"), (0.1, "hit.slash"), (0.6, "swing"), (0.7, "hit.slash"), (0.7, "beat"), (1.2, "swing"), (1.3, "hit.slash"), (1.3, "beat"), (1.8, "swing"), (1.9, "hit.slash"), (1.9, "beat"), (1.9, "flourish"), (2.4, "swing"), (2.5, "hit.slash"), (2.5, "kill")],
            // A mace on a shelled creature, a dagger's quick stabs.
            &[(0.0, "swing.mace"), (0.18, "hit.crush"), (0.18, "hit.shell"), (0.9, "swing.dagger"), (0.96, "hit.stab"), (1.3, "swing.dagger"), (1.36, "hit.stab"), (1.7, "swing.dagger"), (1.76, "hit.stab"), (1.76, "beat")],
            // A bow, and a fist that misses.
            &[(0.0, "swing.bow"), (0.35, "hit.ranged"), (1.0, "swing.fist"), (1.08, "miss")],
            // A creature's blows: one that hurts, one blocked, a perfect block and its counter.
            &[(0.0, "hurt"), (0.8, "guard"), (1.0, "blocked"), (1.8, "guard"), (2.0, "perfect"), (2.3, "swing"), (2.4, "hit.slash"), (2.4, "counter")],
            // A dodge and the blow it let pass.
            &[(0.0, "dodge"), (0.12, "miss")],
            // Bleeding.
            &[(0.0, "bleed"), (0.6, "bleed"), (1.2, "bleed")],
            &[],
        ];
        for scene in scenes {
            let start = all.len() as f32 / SR + 0.3;
            for &(at, name) in scene {
                put(&mut all, start + at, name);
            }
            all.extend(std::iter::repeat_n(0.0, (0.6 * SR) as usize));
        }
        for name in look.sounds.keys() {
            println!("{:6.2} s  {name}", all.len() as f32 / SR);
            let at = all.len() as f32 / SR;
            put(&mut all, at, Box::leak(name.clone().into_boxed_str()));
            all.extend(std::iter::repeat_n(0.0, (0.45 * SR) as usize));
        }
        let peak = all.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        std::fs::write(path, crate::sfx::wav(&all.iter().map(|s| s * 0.9 / peak.max(1.0)).collect::<Vec<_>>())).unwrap();
    }

    #[test]
    fn sounds_are_deterministic() {
        let look = look();
        assert_eq!(render(&look.sounds["hit.slash"], 3), render(&look.sounds["hit.slash"], 3));
    }

    #[test]
    fn heavy_weapons_whoosh_lower_and_longer_and_crushing_hits_thump_deeper() {
        let look = look();
        let (dagger, mace) = (take(&look.sounds["swing.dagger"], 1), take(&look.sounds["swing.mace"], 1));
        assert!(mace.len() > dagger.len(), "a mace's swing is longer than a dagger's");
        let low_share = |s: &[f32]| energy(s, 150.0, 900.0) / energy(s, 900.0, 5000.0).max(1e-6);
        assert!(low_share(&mace) > low_share(&dagger), "a mace whooshes lower than a dagger");
        let (stab, crush) = (take(&look.sounds["hit.stab"], 1), take(&look.sounds["hit.crush"], 1));
        let deep = |s: &[f32]| energy(s, 30.0, 120.0) / energy(s, 30.0, 6000.0).max(1e-6);
        assert!(deep(&crush) > deep(&stab), "a crushing hit is deeper than a stab");
        // A perfect block rings on: more of it is left after a tenth of a second than of a hit.
        let tail = |s: &[f32]| s.iter().skip((0.1 * SR) as usize).map(|x| x * x).sum::<f32>() / s.iter().map(|x| x * x).sum::<f32>();
        assert!(tail(&take(&look.sounds["perfect"], 1)) > tail(&take(&look.sounds["hit.stab"], 1)));
    }
}
