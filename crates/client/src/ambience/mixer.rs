//! The ambience mixer: frame by frame, from the environment, how loud each bed is, when
//! its next segment starts, and which one-shots play, where and how loud. It knows nothing
//! of Bevy, so the game and the offline preview drive it the same way.
//!
//! - A bed follows its conditions' weight at a steady rate (full scale over its `fade`
//!   seconds), turned down by whatever ducks it. While audible it plays segments back to
//!   back, overlapping by [`OVERLAP`], never the same variant twice in a row.
//! - A one-shot plays at random, at its rate per minute scaled by how well its conditions
//!   hold, then waits out its cooldown. Ducking turns it down rather than silencing it.
//!   Nothing plays in the first seconds after joining, and only so many at once.

use bevy::math::{IVec3, Vec3};

use super::defs::{Ambience, DuckBy, Environment, Fixed, OPEN_AIR_HZ, Placement};
use super::synth::{OVERLAP, SEGMENT};
use crate::sfx::mix;

/// Below this a bed is silent and stops.
const SILENT: f32 = 1e-3;
/// Seconds after starting before one-shots may play.
const WARM_UP: f64 = 4.0;
/// One-shots sounding at once, at most.
const MAX_SOUNDING: usize = 8;

/// Something for the player to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cue {
    /// Start this variant of a bed's segments now.
    Segment { bed: usize, variant: usize },
    /// The bed has faded out: stop its segments.
    Stop { bed: usize },
    /// Play a one-shot at a place (`None`: at the listener), at a volume and speed, muffled
    /// by 0..1 (heard from indoors: [`muffle`]).
    Sound { sound: usize, variant: usize, at: Option<Vec3>, volume: f32, speed: f32, muffle: f32 },
}

#[derive(Clone, Copy, Default)]
struct BedState {
    level: f32,
    /// When the next segment starts, while the bed is playing.
    next_at: Option<f64>,
    last: Option<usize>,
}

pub struct Mixer {
    beds: Vec<BedState>,
    /// When each one-shot may next play.
    ready_at: Vec<f64>,
    /// When the one-shots sounding now end.
    sounding: Vec<f64>,
    now: f64,
    random: u64,
}

impl Mixer {
    pub fn new(ambience: &Ambience, seed: u64) -> Self {
        Self {
            beds: vec![BedState::default(); ambience.beds.len()],
            ready_at: vec![WARM_UP; ambience.sounds.len()],
            sounding: Vec::new(),
            now: 0.0,
            random: seed,
        }
    }

    /// A bed's volume now, before the master volume.
    pub fn level(&self, bed: usize) -> f32 {
        self.beds[bed].level
    }

    fn unit(&mut self) -> f32 {
        self.random = mix(self.random);
        (self.random >> 40) as f32 / (1u64 << 24) as f32
    }

    fn pick(&mut self, count: usize, not: Option<usize>) -> usize {
        let choices = if not.is_some() && count > 1 { count - 1 } else { count };
        let i = ((self.unit() * choices as f32) as usize).min(choices - 1);
        match not {
            Some(n) if count > 1 && i >= n => i + 1,
            _ => i,
        }
    }

    /// How much `duck` lets through.
    fn ducking(&self, ambience: &Ambience, env: &Environment, duck: &[(DuckBy, f32)]) -> f32 {
        duck.iter()
            .map(|&(by, amount)| {
                let strength = match by {
                    DuckBy::Axis(axis) => env.values[axis].clamp(0.0, 1.0),
                    DuckBy::Bed(bed) => self.beds[bed].level / ambience.beds[bed].volume.max(1e-6),
                };
                1.0 - amount * strength
            })
            .product()
    }

    /// How muffled a layer is, 0..1: as far as it ducks indoors.
    pub fn muffling(&self, env: &Environment, duck: &[(DuckBy, f32)]) -> f32 {
        let shelter = env.get(Fixed::Shelter).clamp(0.0, 1.0);
        duck.iter().filter(|(by, _)| *by == DuckBy::Axis(Fixed::Shelter as usize)).map(|&(_, amount)| amount * shelter).fold(0.0, f32::max)
    }

    /// Advances by `dt` seconds. `samples` are the scan's blocks per group, `listener` the
    /// player's ears, and `length(sound, variant)` a one-shot's length in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        ambience: &Ambience,
        env: &Environment,
        samples: &[Vec<IVec3>],
        buildings: &[Vec<Vec3>],
        listener: Vec3,
        dt: f32,
        length: impl Fn(usize, usize) -> f32,
        variants: (usize, usize),
        cues: &mut Vec<Cue>,
    ) {
        self.now += dt as f64;
        let (bed_variants, sound_variants) = variants;
        // Ducking by beds uses last frame's levels, so the order of beds does not matter.
        let ducks: Vec<f32> = ambience.beds.iter().map(|bed| self.ducking(ambience, env, &bed.duck)).collect();
        for (i, bed) in ambience.beds.iter().enumerate() {
            let target = bed.volume * bed.when.weight(env) * ducks[i];
            let step = bed.volume / bed.fade * dt;
            let state = &mut self.beds[i];
            state.level += (target - state.level).clamp(-step, step);
            if state.level > SILENT {
                if state.next_at.is_none_or(|at| self.now >= at) {
                    let last = self.beds[i].last;
                    let variant = self.pick(bed_variants, last);
                    let state = &mut self.beds[i];
                    state.last = Some(variant);
                    state.next_at = Some(self.now + (SEGMENT - OVERLAP) as f64);
                    cues.push(Cue::Segment { bed: i, variant });
                }
            } else if state.next_at.take().is_some() {
                cues.push(Cue::Stop { bed: i });
            }
        }

        let now = self.now;
        self.sounding.retain(|&end| end > now);
        for (i, sound) in ambience.sounds.iter().enumerate() {
            if now < self.ready_at[i] || self.sounding.len() >= MAX_SOUNDING {
                continue;
            }
            let weight = sound.when.weight(env);
            if weight <= 0.0 || self.unit() >= sound.per_minute / 60.0 * weight * dt {
                continue;
            }
            let at = match sound.at {
                Placement::Block(group) => {
                    let Some(blocks) = samples.get(group).filter(|b| !b.is_empty()) else { continue };
                    let block = blocks[self.pick(blocks.len(), None)];
                    Some(block.as_vec3() + 0.5)
                }
                Placement::Around { distance, height } => {
                    let angle = self.unit() * std::f32::consts::TAU;
                    let d = distance.0 + (distance.1 - distance.0) * self.unit();
                    let h = height.0 + (height.1 - height.0) * self.unit();
                    Some(listener + Vec3::new(angle.cos() * d, h, angle.sin() * d))
                }
                Placement::Listener => None,
                Placement::Building(set) => {
                    let Some(places) = buildings.get(set).filter(|p| !p.is_empty()) else { continue };
                    Some(places[self.pick(places.len(), None)])
                }
            };
            let distance = at.map_or(0.0, |at| at.distance(listener));
            let volume = sound.volume * (0.6 + 0.4 * weight) * self.ducking(ambience, env, &sound.duck) * sound.reach / (sound.reach + distance);
            let speed = sound.pitch.0 + (sound.pitch.1 - sound.pitch.0) * self.unit();
            let variant = self.pick(sound_variants, None);
            self.ready_at[i] = now + sound.cooldown as f64;
            self.sounding.push(now + (length(i, variant) / speed) as f64);
            let muffle = self.muffling(env, &sound.duck);
            cues.push(Cue::Sound { sound: i, variant, at, volume, speed, muffle });
        }
    }
}

/// A sample a clip is kept in: a float, or 16 bits (full scale at ±32767).
pub trait Sample: Copy {
    fn to_f32(self) -> f32;
    fn from_f32(value: f32) -> Self;
}

impl Sample for f32 {
    fn to_f32(self) -> f32 {
        self
    }
    fn from_f32(value: f32) -> Self {
        value
    }
}

impl Sample for i16 {
    fn to_f32(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
    fn from_f32(value: f32) -> Self {
        (value.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
    }
}

/// Places a mono one-shot in stereo for a listener at `ears`, facing `forward` with
/// `right` to the right, and blends in its reverb `wet` by `reverb`. Sources behind are a
/// little quieter, as the ears' shadow makes them; at the listener it sounds from the
/// middle.
pub fn place<S: Sample>(dry: &[S], wet: &[Vec<S>; 2], at: Option<Vec3>, ears: Vec3, right: Vec3, forward: Vec3, reverb: f32) -> [Vec<f32>; 2] {
    let (left_gain, right_gain) = match at {
        None => (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2),
        Some(at) => {
            let to = at - ears;
            let distance = to.length().max(0.5);
            let pan = (to.dot(right) / distance).clamp(-1.0, 1.0);
            let behind = 1.0 - 0.25 * (-to.dot(forward) / distance).max(0.0);
            let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            (angle.cos() * behind, angle.sin() * behind)
        }
    };
    let dry_gain = 1.0 - 0.35 * reverb;
    let len = dry.len().max(wet[0].len());
    let channel = |gain: f32, wet: &[S]| -> Vec<f32> {
        (0..len).map(|i| dry.get(i).map_or(0.0, |s| s.to_f32()) * gain * dry_gain + wet.get(i).map_or(0.0, |s| s.to_f32()) * reverb).collect()
    };
    [channel(left_gain, &wet[0]), channel(right_gain, &wet[1])]
}

/// Every how many samples a fully muffled bed keeps one: past its cutoff there is nothing
/// left to keep at the full rate, and a bed so kept takes a quarter of the memory.
pub const MUFFLED_STEP: usize = 4;

/// A bed's segment fully muffled, as heard from indoors: low-passed to `cutoff` Hz and
/// kept at a [`MUFFLED_STEP`]th of the rate.
pub fn muffled(mut channels: [Vec<f32>; 2], cutoff: f32) -> [Vec<f32>; 2] {
    muffle(&mut channels, 1.0, cutoff.min(crate::sfx::RATE as f32 / MUFFLED_STEP as f32 * 0.4));
    channels.map(|channel| channel.into_iter().step_by(MUFFLED_STEP).collect())
}

/// Muffles a placed one-shot by `amount` (0: as it is, 1: down to `cutoff` Hz) with a gentle
/// low-pass, as a wall between makes it: the cutoff falls evenly in pitch from open air.
pub fn muffle(channels: &mut [Vec<f32>; 2], amount: f32, cutoff: f32) {
    if amount <= 0.0 || cutoff >= OPEN_AIR_HZ {
        return;
    }
    let hz = OPEN_AIR_HZ * (cutoff / OPEN_AIR_HZ).powf(amount.min(1.0));
    let a = 1.0 - (-std::f32::consts::TAU * hz / crate::sfx::RATE as f32).exp();
    for channel in channels {
        // Two one-pole passes: 12 dB an octave, soft enough to keep the sound's shape.
        for _ in 0..2 {
            let mut y = 0.0;
            for s in channel.iter_mut() {
                y += a * (*s - y);
                *s = y;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs::FILE;
    use super::*;
    use voxmar_engine::content::ContentFile;

    fn ambience() -> Ambience {
        Ambience::load(&[ContentFile {
            package: "test".into(),
            name: FILE.into(),
            source: r#"(
                groups: [(name: "leaves", tags: ["leaves"], range: (0, 1))],
                beds: [(name: "rustle", sound: Rustle, volume: 0.5, fade: 2, when: {"leaves": (min: 0.5)}, duck: {"rain": 0.8})],
                sounds: [(name: "bird", sound: Warbler, volume: 0.4, at: Block("leaves"), per_minute: 30, cooldown: 1, reach: 10)],
            )"#
            .into(),
        }])
        .unwrap()
    }

    fn run(mixer: &mut Mixer, ambience: &Ambience, env: &Environment, samples: &[Vec<IVec3>], seconds: f32) -> Vec<Cue> {
        let mut cues = Vec::new();
        for _ in 0..(seconds * 60.0) as usize {
            mixer.update(ambience, env, samples, &[], Vec3::ZERO, 1.0 / 60.0, |_, _| 1.0, (3, 5), &mut cues);
        }
        cues
    }

    #[test]
    fn beds_fade_chain_segments_and_stop() {
        let ambience = ambience();
        let mut mixer = Mixer::new(&ambience, 1);
        let mut env = Environment::new(1);
        env.set_group(0, 1.0);
        let cues = run(&mut mixer, &ambience, &env, &[], 1.0);
        assert!(matches!(cues.first(), Some(Cue::Segment { bed: 0, .. })));
        assert!((mixer.level(0) - 0.25).abs() < 0.02, "half way after half the fade: {}", mixer.level(0));
        // Over a minute segments follow each other every SEGMENT - OVERLAP seconds and
        // never repeat a variant back to back.
        let cues = run(&mut mixer, &ambience, &env, &[], 60.0);
        let variants: Vec<usize> = cues.iter().filter_map(|c| if let Cue::Segment { variant, .. } = c { Some(*variant) } else { None }).collect();
        assert_eq!(variants.len(), (60.0 / (SEGMENT - OVERLAP)) as usize);
        assert!(variants.windows(2).all(|w| w[0] != w[1]));
        assert!((mixer.level(0) - 0.5).abs() < 1e-6);
        // Rain ducks the bed to a fifth of its volume rather than silencing it.
        env.set(Fixed::Rain, 1.0);
        run(&mut mixer, &ambience, &env, &[], 5.0);
        assert!((mixer.level(0) - 0.1).abs() < 1e-3);
        // Leaving the trees fades it out and stops it.
        env.set_group(0, 0.0);
        let cues = run(&mut mixer, &ambience, &env, &[], 3.0);
        assert_eq!(cues.last(), Some(&Cue::Stop { bed: 0 }));
        assert_eq!(mixer.level(0), 0.0);
    }

    #[test]
    fn one_shots_sound_from_real_blocks_at_their_rate() {
        let ambience = ambience();
        let mut mixer = Mixer::new(&ambience, 2);
        let mut env = Environment::new(1);
        env.set_group(0, 1.0);
        // Without a block to sing from, birds stay quiet.
        assert!(run(&mut mixer, &ambience, &env, &[vec![]], 20.0).iter().all(|c| !matches!(c, Cue::Sound { .. })));
        let tree = IVec3::new(10, 4, 0);
        let cues = run(&mut mixer, &ambience, &env, &[vec![tree]], 120.0);
        let sounds: Vec<_> = cues.iter().filter_map(|c| if let Cue::Sound { at, volume, .. } = c { Some((*at, *volume)) } else { None }).collect();
        // 30 a minute, less the time spent cooling down after each.
        assert!((25..=60).contains(&sounds.len()), "{}", sounds.len());
        let (at, volume) = sounds[0];
        assert_eq!(at, Some(tree.as_vec3() + 0.5));
        assert!(volume < 0.4 * 0.6, "farther than its reach, so under half as loud");
    }

    #[test]
    fn nothing_plays_while_warming_up() {
        let ambience = ambience();
        let mut mixer = Mixer::new(&ambience, 3);
        let mut env = Environment::new(1);
        env.set_group(0, 1.0);
        let cues = run(&mut mixer, &ambience, &env, &[vec![IVec3::ZERO]], WARM_UP as f32 - 0.1);
        assert!(cues.iter().all(|c| !matches!(c, Cue::Sound { .. })));
    }

    #[test]
    fn placing_pans_by_direction() {
        let dry = vec![1.0; 10];
        let wet = [vec![0.0; 20], vec![0.0; 20]];
        let [l, r] = place(&dry, &wet, Some(Vec3::new(5.0, 0.0, 0.0)), Vec3::ZERO, Vec3::X, Vec3::NEG_Z, 0.0);
        assert!(r[0] > 0.99 && l[0] < 0.01, "a source on the right is heard on the right");
        assert_eq!(l.len(), 20);
        let [l, r] = place(&dry, &wet, None, Vec3::ZERO, Vec3::X, Vec3::NEG_Z, 0.0);
        assert!((l[0] - r[0]).abs() < 1e-6);
    }

    /// Indoors an outdoor one-shot loses its brightness, not its body; out in the open it
    /// is left as it is.
    #[test]
    fn muffling_darkens_without_silencing() {
        let tone = |hz: f32| -> [Vec<f32>; 2] {
            let wave: Vec<f32> = (0..crate::sfx::RATE as usize / 2).map(|i| (i as f32 * std::f32::consts::TAU * hz / crate::sfx::RATE as f32).sin()).collect();
            [wave.clone(), wave]
        };
        let rms = |c: &[Vec<f32>; 2]| (c[0][c[0].len() / 2..].iter().map(|s| s * s).sum::<f32>() / (c[0].len() / 2) as f32).sqrt();
        let heard = |hz: f32, amount: f32| {
            let mut c = tone(hz);
            muffle(&mut c, amount, 700.0);
            rms(&c) / rms(&tone(hz))
        };
        assert_eq!(heard(5000.0, 0.0), 1.0);
        assert!(heard(150.0, 1.0) > 0.9, "the body: {}", heard(150.0, 1.0));
        assert!(heard(5000.0, 1.0) < 0.05, "the brightness: {}", heard(5000.0, 1.0));
        assert!(heard(5000.0, 0.5) > heard(5000.0, 1.0) && heard(5000.0, 0.5) < 0.9, "half indoors, half muffled");
    }
}
