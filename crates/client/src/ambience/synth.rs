//! The sounds of the world, synthesized at startup like the sound effects (`crate::sfx`):
//! nothing is downloaded or shipped, and every machine hears the same thing.
//!
//! Beds (wind, leaves, insects, water, caves) are made of [`SEGMENT`]-second stereo
//! segments that fade in and out over [`OVERLAP`] seconds with equal power, so playing
//! different variants back to back, overlapping by that much, gives an endless sound with
//! no seam and nothing that audibly loops. The two channels are made from different seeds,
//! so a bed is wide and diffuse rather than coming from the middle of the head.
//!
//! One-shots (birdsong, an owl, a frog, a drip) are short and mono. The mixer places them
//! in the stereo field when they play, and blends in a reverb tail rendered with each, as
//! wet as the place is cave-like.

use std::f32::consts::{PI, TAU};

use serde::Deserialize;

use crate::sfx::dsp::{Drift, Pink, Svf, between, bubble, gap, seconds, tap};
use crate::sfx::{Noise, RATE, mix};

const SR: f32 = RATE as f32;
/// Seconds in one segment of a bed.
pub const SEGMENT: f32 = 8.0;
/// Seconds over which consecutive segments of a bed overlap.
pub const OVERLAP: f32 = 1.5;
/// Variants made of every bed and every one-shot.
pub const BED_VARIANTS: usize = 3;
pub const ONE_SHOT_VARIANTS: usize = 5;
/// Seconds of reverb after a one-shot ends.
const REVERB_TAIL: f32 = 2.2;

/// A synthesizer. The first group make beds, the rest one-shots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Synth {
    /// A soft breeze in the open: low, slowly gusting air.
    Wind,
    /// Strong wind on heights and in storms: rougher, brighter, with a faint whistle.
    Gale,
    /// Leaves shimmering in the breeze, busier in the gusts.
    Rustle,
    /// A warm day in grassland: a soft hiss of grass and distant grasshoppers.
    Meadow,
    /// A night chorus of crickets.
    Crickets,
    /// A babbling stream.
    Stream,
    /// Waves breaking on a shore.
    Surf,
    /// The low, still air of a cave, with a faint draught.
    Cave,
    /// Hail on the ground and on everything around: hard, bright ticks and rattles, coming
    /// in waves.
    Hail,
    /// Sleet: a fine, icy patter over a soft, slushy hiss.
    Sleet,
    /// Sand and dust blown hard: a dry, grainy hiss over a roar, rising with the gusts.
    Sandstorm,
    /// Lava: slow, thick bubbles that swell and burst low, the crust ticking and crackling
    /// as it cools, over a deep seething.
    Lava,
    /// A waterfall: a broad, steady roar with a low thunder of the plunge and a hiss of
    /// spray, surging a little.
    Waterfall,
    /// A wood fire: a soft breathing of flame over a warm low roar, busy with small dry
    /// crackles and the odd louder tick.
    Fire,
    /// Rain on a thatched roof: a soft, close hush of drops sinking into straw, the odd
    /// fuller drop among them.
    ThatchPatter,
    /// Rain on a stone roof overhead: dull knocks and a low wash of water over the stone.
    StonePatter,
    /// Rain on glass overhead: bright, ringing ticks over a thin trickle.
    GlassPatter,
    /// Wind finding the gaps in a draughty wall: a thin, wavering whistle that comes and
    /// goes with the gusts, over a faint breathy hiss.
    Draught,
    /// Thatch or wattle smouldering, about to catch: a soft, close sizzling hiss, swelling
    /// and easing, with a fine fizz of tiny ticks through it.
    Smoulder,
    /// Thatch or wattle burning: a crisp, busy crackle of straw and withies going up over a
    /// light roar, brighter and quicker than a hearth's logs.
    Blaze,
    /// A settlement's people about their day, heard across it: many voices too far to make
    /// out, talking in turns, now and then a laugh or a call over the rest.
    Murmur,
    /// A quick songbird: trills and twitters.
    Warbler,
    /// A fluting songbird that repeats its phrases.
    Thrush,
    /// Two or three clear whistled notes falling in pitch ("fee-bee").
    Whistler,
    /// A dove's soft, low cooing.
    Dove,
    /// A woodpecker drumming on a trunk.
    Woodpecker,
    /// An owl's hoots.
    Owl,
    /// One cricket chirping nearby.
    Cricket,
    /// A frog's croaks.
    Frog,
    /// Water: a plop and a gurgle of bubbles.
    Plop,
    /// A gust of wind rising and passing.
    Gust,
    /// A drop of water falling in a cave.
    Drip,
    /// A deep, distant rumble from the rock.
    Rumble,
    /// Grit loosed in a dry cave: a few fine stones ticking down the rock, a breath of dust.
    Grit,
    /// A trunk or branch creaking in the wind.
    Creak,
    /// A bubble of lava bursting: a thick, low blurp and a spatter.
    LavaPop,
    /// A log in a fire snapping: a sharp crack, a few smaller ones after, and a breath of
    /// flame.
    Snap,
    /// A pot on the boil: a run of small, round bubbles breaking at the top.
    Simmer,
    /// Someone at work with a hammer: a few blows on timber, a nail ringing now and then.
    Hammer,
    /// A saw worked through a board: rasping strokes back and forth, the push harder.
    Saw,
    /// A hen pottering about: a run of short clucks, now and then a rising "ba-gawk".
    Hen,
    /// A bucket drawn at a well: the winch creaking, water sloshing, the bucket knocking
    /// the rim.
    Well,
    /// A wooden door opened and shut: the latch lifted, a short creak on its hinges, and
    /// the door knocking home in its frame.
    Door,
    /// A dog barking once to thrice, a good way off.
    Bark,
    /// A sheep bleating in a pen, wavering.
    Bleat,
    /// Supper things: a pot lid and a ladle knocking, a bowl set down.
    Clatter,
    /// Bees about their hive: a warm drone of many wings, one or two passing close.
    Hum,
    /// The smith at the anvil: a heavy blow and a light one or two, iron ringing bright
    /// and long.
    Anvil,
    /// A small shrine bell rung two or three times, its hum lingering.
    Bell,
    /// A mill wheel turning: its paddles knocking round, the axle groaning, water
    /// spilling off.
    Mill,
    /// Wood split at a woodyard: an axe biting a log two or three times, the last
    /// splitting it and the halves knocking down.
    Chop,
    /// A log settling in the fire: a soft fall, a burst of sparks and a breath of
    /// hiss.
    Settle,
    /// A pig rooting in its pen: a few low grunts, now and then a snort.
    Grunt,
}

impl Synth {
    #[cfg(test)]
    pub const ALL: [Synth; 53] = [
        Synth::Wind,
        Synth::Gale,
        Synth::Rustle,
        Synth::Meadow,
        Synth::Crickets,
        Synth::Stream,
        Synth::Surf,
        Synth::Cave,
        Synth::Hail,
        Synth::Sleet,
        Synth::Sandstorm,
        Synth::Lava,
        Synth::Waterfall,
        Synth::Fire,
        Synth::ThatchPatter,
        Synth::StonePatter,
        Synth::GlassPatter,
        Synth::Draught,
        Synth::Smoulder,
        Synth::Blaze,
        Synth::Murmur,
        Synth::Warbler,
        Synth::Thrush,
        Synth::Whistler,
        Synth::Dove,
        Synth::Woodpecker,
        Synth::Owl,
        Synth::Cricket,
        Synth::Frog,
        Synth::Plop,
        Synth::Gust,
        Synth::Drip,
        Synth::Rumble,
        Synth::Grit,
        Synth::Creak,
        Synth::LavaPop,
        Synth::Snap,
        Synth::Simmer,
        Synth::Hammer,
        Synth::Saw,
        Synth::Hen,
        Synth::Well,
        Synth::Door,
        Synth::Bark,
        Synth::Bleat,
        Synth::Clatter,
        Synth::Hum,
        Synth::Anvil,
        Synth::Bell,
        Synth::Mill,
        Synth::Chop,
        Synth::Settle,
        Synth::Grunt,
    ];

    /// Whether it makes a continuous bed rather than one-shots.
    pub fn is_bed(self) -> bool {
        matches!(
            self,
            Synth::Wind
                | Synth::Gale
                | Synth::Rustle
                | Synth::Meadow
                | Synth::Crickets
                | Synth::Stream
                | Synth::Surf
                | Synth::Cave
                | Synth::Hail
                | Synth::Sleet
                | Synth::Sandstorm
                | Synth::Lava
                | Synth::Waterfall
                | Synth::Fire
                | Synth::ThatchPatter
                | Synth::StonePatter
                | Synth::GlassPatter
                | Synth::Draught
                | Synth::Smoulder
                | Synth::Blaze
                | Synth::Murmur
                | Synth::Hum
        )
    }

    fn seed(self, variant: usize) -> u64 {
        mix(0xa3b1_e5c7 ^ (self as u64) << 20 ^ variant as u64)
    }
}

/// One bed segment, `[left, right]`, with its equal-power fades.
pub fn bed(synth: Synth, variant: usize) -> [Vec<f32>; 2] {
    let len = (SEGMENT * SR) as usize;
    let seed = synth.seed(variant);
    let a = render_bed(synth, &mut Noise(seed), len);
    let b = render_bed(synth, &mut Noise(mix(seed ^ 0x5bd1_e995)), len);
    // Mostly independent channels, with a little shared, keep the bed's power.
    let (c, s) = (0.35f32.cos(), 0.35f32.sin());
    let mut left: Vec<f32> = a.iter().zip(&b).map(|(a, b)| a * c + b * s).collect();
    let mut right: Vec<f32> = a.iter().zip(&b).map(|(a, b)| b * c + a * s).collect();
    let scale = loudness(&left, 0.15, 0.9).min(loudness(&right, 0.15, 0.9));
    let fade = (OVERLAP * SR) as usize;
    for channel in [&mut left, &mut right] {
        for (i, sample) in channel.iter_mut().enumerate() {
            let edge = i.min(len - 1 - i);
            let w = if edge < fade { (PI / 2.0 * edge as f32 / fade as f32).sin() } else { 1.0 };
            *sample *= scale * w;
        }
    }
    [left, right]
}

/// One variant of a one-shot: the dry sound, and its reverb, `[left, right]`, which starts
/// with the sound and rings on after it.
pub fn one_shot(synth: Synth, variant: usize) -> (Vec<f32>, [Vec<f32>; 2]) {
    let mut noise = Noise(synth.seed(variant));
    let mut dry = match synth {
        Synth::Warbler => warbler(&mut noise),
        Synth::Thrush => thrush(&mut noise),
        Synth::Whistler => whistler(&mut noise),
        Synth::Dove => dove(&mut noise),
        Synth::Woodpecker => woodpecker(&mut noise),
        Synth::Owl => owl(&mut noise),
        Synth::Cricket => cricket(&mut noise),
        Synth::Frog => frog(&mut noise),
        Synth::Plop => plop(&mut noise),
        Synth::Gust => gust(&mut noise),
        Synth::Drip => drip(&mut noise),
        Synth::Rumble => rumble(&mut noise),
        Synth::Grit => grit(&mut noise),
        Synth::Creak => creak(&mut noise),
        Synth::LavaPop => lava_pop(&mut noise),
        Synth::Snap => snap(&mut noise),
        Synth::Simmer => simmer(&mut noise),
        Synth::Hammer => hammer(&mut noise),
        Synth::Saw => saw(&mut noise),
        Synth::Hen => hen(&mut noise),
        Synth::Well => well(&mut noise),
        Synth::Door => door(&mut noise),
        Synth::Bark => bark(&mut noise),
        Synth::Bleat => bleat(&mut noise),
        Synth::Clatter => clatter(&mut noise),
        Synth::Anvil => anvil(&mut noise),
        Synth::Bell => bell(&mut noise),
        Synth::Mill => mill(&mut noise),
        Synth::Chop => chop(&mut noise),
        Synth::Settle => settle(&mut noise),
        Synth::Grunt => grunt(&mut noise),
        bed => panic!("{bed:?} makes a bed, not one-shots"),
    };
    let scale = loudness(&dry, 0.14, 0.8);
    dry.iter_mut().for_each(|s| *s *= scale);
    fade_ends(&mut dry, 0.002, 0.01);
    let wet = reverb(&dry);
    (dry, wet)
}

fn render_bed(synth: Synth, noise: &mut Noise, len: usize) -> Vec<f32> {
    match synth {
        Synth::Wind => wind(noise, len, false),
        Synth::Gale => wind(noise, len, true),
        Synth::Rustle => rustle(noise, len),
        Synth::Meadow => meadow(noise, len),
        Synth::Crickets => crickets(noise, len),
        Synth::Stream => stream(noise, len),
        Synth::Surf => surf(noise, len),
        Synth::Cave => cave(noise, len),
        Synth::Hail => hail(noise, len),
        Synth::Sleet => sleet(noise, len),
        Synth::Sandstorm => sandstorm(noise, len),
        Synth::Lava => lava(noise, len),
        Synth::Hum => hum(noise, len),
        Synth::Waterfall => waterfall(noise, len),
        Synth::Fire => fire(noise, len),
        Synth::ThatchPatter => thatch_patter(noise, len),
        Synth::StonePatter => stone_patter(noise, len),
        Synth::GlassPatter => glass_patter(noise, len),
        Synth::Draught => draught(noise, len),
        Synth::Smoulder => smoulder(noise, len),
        Synth::Blaze => blaze(noise, len),
        Synth::Murmur => murmur(noise, len),
        one_shot => panic!("{one_shot:?} makes one-shots, not a bed"),
    }
}

// --- Building blocks ---

/// A sine whose frequency may change every sample.
#[derive(Default)]
struct Osc(f32);

impl Osc {
    fn next(&mut self, frequency: f32) -> f32 {
        self.0 = (self.0 + frequency / SR).fract();
        (TAU * self.0).sin()
    }
}

/// Adds `count` samples from `f(j)` into `out` starting at time `t`, which may be before
/// the start (those samples are dropped).
fn add_at(out: &mut [f32], t: f32, count: usize, mut f: impl FnMut(usize) -> f32) {
    let first = (t * SR).round() as i64;
    for j in 0..count {
        let at = first + j as i64;
        let value = f(j);
        if let Some(sample) = usize::try_from(at).ok().and_then(|at| out.get_mut(at)) {
            *sample += value;
        }
    }
}

/// A smooth bump over 0..1 with a quicker rise than fall.
fn note_envelope(x: f32, attack: f32) -> f32 {
    if !(0.0..1.0).contains(&x) {
        return 0.0;
    }
    let rise = (x / attack).min(1.0);
    let fall = ((1.0 - x) / (1.0 - attack)).min(1.0);
    (rise * PI / 2.0).sin().powi(2) * fall.sqrt()
}

/// Adds a tone of `length` seconds at `start` into `out`, gliding from `from` to `to` Hz,
/// with relative `harmonics` above the fundamental.
#[allow(clippy::too_many_arguments)]
fn tone(out: &mut Vec<f32>, start: f32, length: f32, from: f32, to: f32, gain: f32, attack: f32, harmonics: &[f32]) {
    let first = seconds(start);
    let count = seconds(length);
    if out.len() < first + count {
        out.resize(first + count, 0.0);
    }
    let mut phase = 0.0f32;
    for i in 0..count {
        let x = i as f32 / count as f32;
        // Glide along a gentle curve rather than a straight line.
        let frequency = from + (to - from) * (x * (2.0 - x));
        phase = (phase + frequency / SR).fract();
        let mut sample = (TAU * phase).sin();
        for (k, h) in harmonics.iter().enumerate() {
            sample += h * (TAU * phase * (k + 2) as f32).sin();
        }
        out[first + i] += gain * note_envelope(x, attack) * sample;
    }
}

/// Scale bringing `samples` to `rms`, unless that would push a peak past `peak`.
fn loudness(samples: &[f32], rms: f32, peak: f32) -> f32 {
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
    let top = samples.iter().fold(1e-6f32, |p, s| p.max(s.abs()));
    (rms / power.sqrt().max(1e-6)).min(peak / top)
}

fn fade_ends(samples: &mut [f32], fade_in: f32, fade_out: f32) {
    let (a, b) = (seconds(fade_in).max(1), seconds(fade_out).max(1));
    let len = samples.len();
    for (i, sample) in samples.iter_mut().enumerate() {
        *sample *= (i as f32 / a as f32).min(1.0) * ((len - i) as f32 / b as f32).min(1.0);
    }
}

/// A cave's reverb of `dry`, from the moment it starts: four damped feedback combs per
/// channel, at slightly different lengths left and right, into two all-passes. The tail
/// holds about half the dry sound's energy and dies away over about two seconds.
fn reverb(dry: &[f32]) -> [Vec<f32>; 2] {
    let len = dry.len() + seconds(REVERB_TAIL);
    let predelay = seconds(0.018);
    let channel = |spread: f32| {
        let mut out = vec![0.0f32; len];
        for delay_ms in [53.1, 61.7, 71.3, 79.9] {
            let delay = seconds((delay_ms + spread) / 1000.0);
            let mut line = vec![0.0f32; delay];
            let mut damped = 0.0f32;
            for (i, sample) in out.iter_mut().enumerate() {
                let input = if i >= predelay { dry.get(i - predelay).copied().unwrap_or(0.0) } else { 0.0 };
                let delayed = line[i % delay];
                damped += 0.35 * (delayed - damped);
                line[i % delay] = input + 0.83 * damped;
                *sample += delayed;
            }
        }
        for (delay_ms, gain) in [(5.0, 0.7), (1.7, 0.7)] {
            let delay = seconds((delay_ms + spread * 0.1) / 1000.0);
            let mut line = vec![0.0f32; delay];
            for (i, sample) in out.iter_mut().enumerate() {
                let delayed = line[i % delay];
                let input = *sample + gain * delayed;
                line[i % delay] = input;
                *sample = delayed - gain * input;
            }
        }
        out
    };
    let mut wet = [channel(0.0), channel(2.3)];
    let energy = |s: &[f32]| s.iter().map(|x| x * x).sum::<f32>();
    let scale = (0.5 * energy(dry) / energy(&wet[0]).max(1e-9)).sqrt();
    for channel in &mut wet {
        channel.iter_mut().for_each(|s| *s *= scale);
        fade_ends(channel, 0.0, 0.3);
    }
    wet
}

// --- Beds ---

/// Air moving over open ground: noise through a band-pass that slides up as gusts swell,
/// over a low body. A gale is brighter and rougher, with a faint whistle.
fn wind(noise: &mut Noise, len: usize, gale: bool) -> Vec<f32> {
    let gust = Drift::new(noise, if gale { 0.12 } else { 0.07 });
    let flutter = Drift::new(noise, 1.3);
    let pitch = Drift::new(noise, 0.07);
    let mut body = Svf::new(300.0, 0.6);
    let mut low = Svf::new(110.0, 0.7);
    let mut whistle = Svf::new(900.0, 5.0);
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let g = gust.unit(t);
            let white = noise.next();
            let (center, roughness) = if gale { (380.0 + 900.0 * g, 0.25) } else { (170.0 + 380.0 * g, 0.1) };
            body.tune(center * (1.0 + 0.08 * flutter.at(t)));
            let mut sample = body.band(white) * (0.2 + g) * (1.0 + roughness * flutter.at(t * 3.0));
            sample += low.low(white) * 0.5 * (0.4 + g);
            if gale {
                whistle.tune(700.0 + 350.0 * pitch.at(t));
                sample += whistle.band(white) * 0.15 * g * g;
            }
            sample * (0.3 + 0.7 * g.powf(1.5))
        })
        .collect()
}

/// Ice striking things: `rate` impacts a second at most (in waves), each a burst of noise
/// `length` seconds long ringing at a random pitch in `band`, some with a duller knock.
fn impacts(noise: &mut Noise, len: usize, rate: f32, length: (f32, f32), band: (f32, f32), knock: f32) -> Vec<f32> {
    let waves = Drift::new(noise, 0.11);
    let mut out = vec![0.0f32; len];
    let mut t = 0.0;
    while t < len as f32 / SR {
        let w = 0.35 + 0.65 * waves.unit(t);
        t += -(1.0 - noise.unit()).max(1e-4).ln() / (rate * w);
        let count = seconds(between(noise, length.0, length.1)).max(2);
        let gain = (0.2 + 0.8 * noise.unit().powi(3)) * w;
        let pitch = between(noise, band.0, band.1);
        let mut ring = Svf::new(pitch, between(noise, 3.0, 8.0));
        let dull = noise.unit() < knock;
        let mut thud = Svf::new(between(noise, 180.0, 420.0), 2.0);
        add_at(&mut out, t, count, |j| {
            let x = j as f32 / count as f32;
            let envelope = (1.0 - x).powi(3);
            let white = noise.next();
            let mut sample = ring.band(white) * envelope;
            if dull {
                sample += thud.band(white) * 1.5 * envelope;
            }
            sample * gain
        });
    }
    // Round off the loudest strikes, so the bed is dense rather than a few sharp cracks.
    let rms = (out.iter().map(|s| s * s).sum::<f32>() / len.max(1) as f32).sqrt().max(1e-6);
    out.iter_mut().for_each(|s| *s = (*s / (3.0 * rms)).tanh() * 3.0 * rms);
    out
}

/// Hail: hard ticks from stones bouncing on ground, rock and leaves, a duller knock now
/// and then, and a rattling hiss of the rest.
fn hail(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = impacts(noise, len, 220.0, (0.004, 0.012), (1800.0, 5200.0), 0.25);
    let mut hiss = Svf::new(4200.0, 0.7);
    let swell = Drift::new(noise, 0.11);
    for (i, sample) in out.iter_mut().enumerate() {
        *sample += hiss.band(noise.next()) * 0.12 * (0.4 + 0.6 * swell.unit(i as f32 / SR));
    }
    out
}

/// Rain on thatch: dense, short, low-pitched drops muffled by the straw, under a soft
/// hush that swells with the rain.
fn thatch_patter(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = impacts(noise, len, 900.0, (0.003, 0.008), (500.0, 1400.0), 0.08);
    let mut hush = Svf::new(900.0, 0.5);
    let swell = Drift::new(noise, 0.09);
    for (i, sample) in out.iter_mut().enumerate() {
        *sample = *sample * 0.6 + hush.band(noise.next()) * 0.35 * (0.5 + 0.5 * swell.unit(i as f32 / SR));
    }
    out
}

/// Wind through a draughty wall: two narrow whistles a little apart, each sliding with its
/// own slow drift, swelling only in the gusts and near silent between, over a faint hiss.
fn draught(noise: &mut Noise, len: usize) -> Vec<f32> {
    let gust = Drift::new(noise, 0.09);
    let (pitch_a, pitch_b) = (Drift::new(noise, 0.13), Drift::new(noise, 0.17));
    let flutter = Drift::new(noise, 2.1);
    let (mut a, mut b) = (Svf::new(900.0, 30.0), Svf::new(1300.0, 26.0));
    let mut hiss = Svf::new(2200.0, 0.7);
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let g = gust.unit(t).powi(3);
            a.tune(820.0 + 260.0 * pitch_a.at(t) + 40.0 * flutter.at(t));
            b.tune(1240.0 + 300.0 * pitch_b.at(t) + 50.0 * flutter.at(t * 1.3));
            let white = noise.next();
            let whistle = a.band(white) * 0.9 + b.band(white) * 0.5;
            (whistle * g + hiss.band(noise.next()) * 0.06 * (0.3 + g)) * (0.15 + 0.85 * g)
        })
        .collect()
}

/// Thatch smouldering: a high, soft sizzle of band noise that swells and eases as the heat
/// comes and goes, a fine fizz of tiny ticks through it, and a low breath under both.
fn smoulder(noise: &mut Noise, len: usize) -> Vec<f32> {
    let swell = Drift::new(noise, 0.25);
    let mut sizzle = Svf::new(4200.0, 1.2);
    let mut breath = Svf::new(500.0, 0.6);
    let mut out: Vec<f32> = (0..len)
        .map(|i| {
            let s = swell.unit(i as f32 / SR);
            sizzle.band(noise.next()) * 0.22 * (0.45 + 0.55 * s) + breath.band(noise.next()) * 0.05 * s
        })
        .collect();
    let mut t = between(noise, 0.0, 0.05);
    while t < len as f32 / SR {
        let gain = 0.03 + 0.08 * noise.unit().powi(3);
        crack(&mut out, noise, t, (0.0005, 0.0015), (3500.0, 7000.0), gain);
        t += gap(noise, 30.0);
    }
    out
}

/// Straw and withies burning: a light roar breathing quickly, and crackles thick and crisp,
/// in quick bursts, the odd louder pop of a withy splitting.
fn blaze(noise: &mut Noise, len: usize) -> Vec<f32> {
    let breath = Drift::new(noise, 0.8);
    let mut roar = Svf::new(260.0, 0.7);
    let mut rush = Svf::new(1400.0, 0.6);
    let mut out: Vec<f32> = (0..len)
        .map(|i| {
            let b = breath.unit(i as f32 / SR);
            let white = noise.next();
            roar.low(white) * (0.35 + 0.3 * b) + rush.band(white) * 0.08 * (0.5 + 0.5 * b)
        })
        .collect();
    let mut t = between(noise, 0.0, 0.1);
    while t < len as f32 / SR {
        let mut at = t;
        for _ in 0..2 + (noise.unit().powi(2) * 9.0) as usize {
            let gain = 0.06 + 0.25 * noise.unit().powi(3);
            crack(&mut out, noise, at, (0.001, 0.003), (2200.0, 6000.0), gain);
            at += between(noise, 0.004, 0.03);
        }
        t = at + gap(noise, 4.0 + 3.0 * breath.unit(t));
    }
    let mut t = between(noise, 0.2, 1.0);
    while t < len as f32 / SR {
        let gain = between(noise, 0.3, 0.5);
        crack(&mut out, noise, t, (0.003, 0.007), (900.0, 1800.0), gain);
        t += gap(noise, 1.1);
    }
    out
}

/// Rain on stone overhead: fewer drops, each a dull knock, over a low wash of water.
fn stone_patter(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = impacts(noise, len, 380.0, (0.006, 0.016), (250.0, 700.0), 0.5);
    let mut wash = Svf::new(420.0, 0.6);
    let swell = Drift::new(noise, 0.07);
    for (i, sample) in out.iter_mut().enumerate() {
        *sample += wash.band(noise.next()) * 0.22 * (0.5 + 0.5 * swell.unit(i as f32 / SR));
    }
    out
}

/// Rain on glass overhead: bright ringing ticks, few and clear, over a thin trickle.
fn glass_patter(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = impacts(noise, len, 260.0, (0.002, 0.006), (3200.0, 7000.0), 0.0);
    let mut trickle = Svf::new(2600.0, 0.8);
    let swell = Drift::new(noise, 0.1);
    for (i, sample) in out.iter_mut().enumerate() {
        *sample += trickle.band(noise.next()) * 0.06 * (0.4 + 0.6 * swell.unit(i as f32 / SR));
    }
    out
}

/// Sleet: many fine, quiet ticks of ice pellets over the slushy hiss of the wet ones.
fn sleet(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = impacts(noise, len, 650.0, (0.0015, 0.004), (3500.0, 6500.0), 0.0);
    let mut slush = Svf::new(1600.0, 0.5);
    let mut fizz = Svf::new(5500.0, 0.6);
    let swell = Drift::new(noise, 0.09);
    for (i, sample) in out.iter_mut().enumerate() {
        let g = 0.5 + 0.5 * swell.unit(i as f32 / SR);
        let white = noise.next();
        *sample = *sample * 0.5 + (slush.band(white) * 0.35 + fizz.band(white) * 0.2) * g;
    }
    out
}

/// A sandstorm: the roar of strong wind, a dry hiss of sand and a dense crackle of grains
/// striking, all surging together with the gusts.
fn sandstorm(noise: &mut Noise, len: usize) -> Vec<f32> {
    let gust = Drift::new(noise, 0.16);
    let flutter = Drift::new(noise, 1.7);
    let mut roar = Svf::new(260.0, 0.7);
    let mut hiss = Svf::new(3500.0, 0.6);
    let mut top = Svf::new(6500.0, 0.8);
    let mut grains = vec![0.0f32; len];
    let mut i = 0;
    while i < len {
        let g = gust.unit(i as f32 / SR);
        if noise.unit() < 0.12 + 0.5 * g {
            let grain = seconds(between(noise, 0.0004, 0.0012)).max(2);
            let strength = 0.3 + 0.7 * noise.unit().powi(2);
            for j in 0..grain.min(len - i) {
                grains[i + j] += strength * (PI * j as f32 / grain as f32).sin();
            }
        }
        i += seconds(0.0007).max(1);
    }
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let g = gust.unit(t);
            let white = noise.next();
            roar.tune(220.0 + 260.0 * g);
            hiss.tune(3000.0 + 1500.0 * g * (1.0 + 0.1 * flutter.at(t)));
            let body = roar.band(white) * (0.3 + 0.7 * g) + hiss.band(white) * 0.45 * (0.4 + 0.6 * g);
            body + top.band(white * grains[i]) * 0.8 * (0.3 + 0.7 * g)
        })
        .collect()
}

/// Leaves: tiny bright grains of noise, many in the gusts and few in the lulls, over a
/// faint hiss.
fn rustle(noise: &mut Noise, len: usize) -> Vec<f32> {
    let gust = Drift::new(noise, 0.09);
    let mut mask = vec![0.0f32; len];
    let step = seconds(0.001);
    let mut i = 0;
    while i < len {
        let g = gust.unit(i as f32 / SR);
        if noise.unit() < (30.0 + 420.0 * g * g) * 0.001 {
            let grain = seconds(between(noise, 0.0015, 0.004)).max(2);
            let strength = 0.2 + 0.8 * noise.unit().powi(2);
            for j in 0..grain.min(len - i) {
                mask[i + j] += strength * (PI * j as f32 / grain as f32).sin().powi(2);
            }
        }
        i += step;
    }
    let mut high = Svf::new(2600.0, 0.7);
    let mut top = Svf::new(6500.0, 0.7);
    let mut hiss = Svf::new(3800.0, 0.6);
    (0..len)
        .map(|i| {
            let g = gust.unit(i as f32 / SR);
            let white = noise.next();
            let grains = top.low(high.high(white * mask[i]));
            grains + hiss.band(noise.next()) * 0.06 * g
        })
        .collect()
}

/// Grasshoppers at a distance: each sings in bursts, a buzz of pulses in a high band, over
/// the soft hiss of grass.
fn meadow(noise: &mut Noise, len: usize) -> Vec<f32> {
    let sway = Drift::new(noise, 0.08);
    let mut hiss = Svf::new(2400.0, 0.5);
    let mut out: Vec<f32> = (0..len).map(|i| hiss.band(noise.next()) * 0.06 * (0.6 + 0.4 * sway.unit(i as f32 / SR))).collect();
    for _ in 0..5 {
        let center = between(noise, 5500.0, 7000.0);
        let pulse_rate = between(noise, 22.0, 40.0);
        let gain = 0.5 + 0.8 * noise.unit().powi(2);
        let mut band = Svf::new(center, 3.0);
        let mut t = -between(noise, 0.0, 3.0);
        while t < len as f32 / SR {
            let burst = between(noise, 0.8, 2.6);
            let count = seconds(burst);
            add_at(&mut out, t, count, |j| {
                let pulse = (PI * pulse_rate * j as f32 / SR).sin().abs().powi(6);
                band.band(noise.next()) * pulse * note_envelope(j as f32 / count as f32, 0.15) * gain
            });
            t += burst + between(noise, 1.0, 4.0);
        }
    }
    out
}

/// A night chorus: crickets at different distances, each chirping three or four quick
/// pulses of a pure high tone at its own steady pace.
fn crickets(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    let mut distant = Svf::new(4500.0, 5.0);
    let swell = Drift::new(noise, 0.15);
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        *sample = distant.band(noise.next()) * 0.08 * (0.5 + 0.5 * swell.unit(t)) * (PI * 2.5 * t).sin().abs();
    }
    for _ in 0..16 {
        let frequency = between(noise, 4000.0, 5000.0);
        let period = between(noise, 0.35, 0.7);
        let pulses = 3 + (noise.unit() * 2.0) as usize;
        let gain = 0.3 + 0.7 * noise.unit().powi(2);
        let mut t = -between(noise, 0.0, period);
        while t < len as f32 / SR {
            for p in 0..pulses {
                let count = seconds(0.016);
                add_at(&mut out, t + p as f32 * 0.03, count, |j| {
                    gain * (PI * j as f32 / count as f32).sin().powi(2) * (TAU * frequency * j as f32 / SR).sin()
                });
            }
            t += period * (0.96 + 0.08 * noise.unit());
        }
    }
    out
}

/// A brook: the soft rush of water over stones (pink noise through a broad band around
/// a kilohertz, over a low body) and the babble of hundreds of small bubbles a second,
/// mostly faint and high, coming in busier and quieter spells (after Farnell's *Designing
/// Sound*). No bubble stands out on its own, so it never turns into the bloop of a cartoon.
fn stream(noise: &mut Noise, len: usize) -> Vec<f32> {
    // How busy the water is, wandering on two time scales so it never pulses evenly.
    let (slow, quick) = (Drift::new(noise, 0.11), Drift::new(noise, 0.7));
    let busy = |t: f32| 0.6 * slow.unit(t) + 0.4 * quick.unit(t);
    let flicker = Drift::new(noise, 3.0);
    let mut pink = Pink::default();
    let mut rush = Svf::new(1000.0, 0.45);
    let mut body = Svf::new(320.0, 0.8);
    let mut out: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let p = pink.next(noise.next());
            rush.band(p) * (0.3 + 0.05 * flicker.at(t) + 0.1 * busy(t)) + body.band(p) * 0.12
        })
        .collect();
    let mut t = 0.0;
    while t < len as f32 / SR {
        let busy = busy(t);
        // Mostly small bubbles (high), a few larger.
        let frequency = 600.0 * 5.0f32.powf(noise.unit().powf(0.7));
        let gain = (0.014 + 0.11 * noise.unit().powi(4)) * (0.5 + busy);
        bubble(&mut out, t, frequency, gain, between(noise, 0.05, 0.3));
        t += gap(noise, 150.0 + 450.0 * busy);
    }
    out
}

/// Surf: waves every several seconds swelling to a crash, the filter opening with each,
/// then a hissing wash, over the low murmur of the sea.
fn surf(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut waves = Vec::new();
    let mut t = -between(noise, 0.0, 9.0);
    while t < len as f32 / SR {
        waves.push((t, between(noise, 1.5, 2.6), 0.6 + 0.4 * noise.unit()));
        t += between(noise, 6.0, 11.0);
    }
    let mut surge = Svf::new(400.0, 0.7);
    let mut wash = Svf::new(3000.0, 0.6);
    let mut murmur = Svf::new(120.0, 0.7);
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let (mut crash, mut tail) = (0.0f32, 0.0f32);
            for &(start, rise, strength) in &waves {
                let x = t - start;
                if x < 0.0 {
                    continue;
                }
                let swell = if x < rise { (x / rise).powi(2) } else { (-(x - rise) / 1.6).exp() };
                crash = crash.max(strength * swell);
                if x > rise {
                    tail = tail.max(strength * (-(x - rise) / 3.5).exp() * (1.0 - (-(x - rise) * 3.0).exp()));
                }
            }
            let white = noise.next();
            surge.tune(250.0 + 2200.0 * crash);
            surge.low(white) * (0.1 + crash) + wash.band(white) * 0.25 * tail + murmur.low(white) * 0.3
        })
        .collect()
}

/// The air of a cave: a very low, slowly moving rumble, one faint resonance of the
/// chamber, and a whisper of draught.
fn cave(noise: &mut Noise, len: usize) -> Vec<f32> {
    let breathe = Drift::new(noise, 0.05);
    let mut deep = Svf::new(80.0, 0.7);
    let mut room = Svf::new(between(noise, 150.0, 220.0), 6.0);
    let mut draught = Svf::new(650.0, 1.5);
    (0..len)
        .map(|i| {
            let b = breathe.unit(i as f32 / SR);
            let white = noise.next();
            deep.low(white) * (0.6 + 0.4 * b) * 1.5 + room.band(white) * 0.15 + draught.band(white) * 0.04 * b
        })
        .collect()
}

/// Lava (after Farnell's bubbles, made thick): slow bubbles low in pitch that swell for a
/// moment and burst, the cooling crust ticking and crackling now and then, over a deep
/// seething rumble.
fn lava(noise: &mut Noise, len: usize) -> Vec<f32> {
    let seethe = Drift::new(noise, 0.2);
    let mut deep = Svf::new(90.0, 0.7);
    let mut body = Svf::new(220.0, 0.9);
    let mut out: Vec<f32> = (0..len)
        .map(|i| {
            let white = noise.next();
            let s = seethe.unit(i as f32 / SR);
            deep.low(white) * (0.55 + 0.3 * s) + body.band(white) * 0.12
        })
        .collect();
    // Bubbles: swelling briefly, then bursting with a low blurp.
    let mut t = between(noise, 0.0, 0.5);
    while t < len as f32 / SR {
        let frequency = between(noise, 90.0, 260.0);
        let gain = between(noise, 0.15, 0.4);
        let swell = between(noise, 0.04, 0.12);
        add_at(&mut out, t, seconds(swell), |j| {
            let x = j as f32 / seconds(swell) as f32;
            0.15 * gain * x * x * (TAU * frequency * 0.7 * j as f32 / SR).sin()
        });
        bubble(&mut out, t + swell, frequency, gain, between(noise, 0.3, 0.8));
        t += gap(noise, 1.6);
    }
    // The crust: sparse dry ticks.
    let mut tick = Svf::new(3000.0, 1.2);
    let mut t = 0.0;
    while t < len as f32 / SR {
        let gain = 0.15 + 0.35 * noise.unit().powi(3);
        let length = between(noise, 0.002, 0.006);
        add_at(&mut out, t, seconds(length * 5.0), |j| gain * tick.band(noise.next()) * (-(j as f32 / SR) / length).exp());
        t += gap(noise, 4.0);
    }
    out
}

/// A wood fire: low, flickering air (the flame drawing), a soft hiss of burning, and
/// crackles: many tiny dry ticks in bursts as the wood spits, and now and then a fuller
/// one. Kept warm and low, so it sits under the music by the hearth.
fn fire(noise: &mut Noise, len: usize) -> Vec<f32> {
    let breath = Drift::new(noise, 0.35);
    let flicker = Drift::new(noise, 3.0);
    let mut roar = Svf::new(160.0, 0.6);
    let mut hiss = Svf::new(1800.0, 0.5);
    let mut out: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let white = noise.next();
            let (b, f) = (breath.unit(t), flicker.unit(t));
            roar.low(white) * (0.5 + 0.35 * b) * (0.8 + 0.2 * f) + hiss.band(white) * 0.03 * (0.4 + 0.6 * b)
        })
        .collect();
    // Crackles: bursts of small ticks, the wood spitting, thicker while the fire breathes in.
    let mut t = between(noise, 0.0, 0.3);
    while t < len as f32 / SR {
        let burst = 1 + (noise.unit().powi(2) * 6.0) as usize;
        let mut at = t;
        for _ in 0..burst {
            let gain = 0.08 + 0.3 * noise.unit().powi(3);
            crack(&mut out, noise, at, (0.0015, 0.005), (1500.0, 4500.0), gain);
            at += between(noise, 0.008, 0.05);
        }
        t = at + gap(noise, 2.2 + 2.0 * breath.unit(t));
    }
    // Now and then a fuller tick, lower and woodier.
    let mut t = between(noise, 0.5, 2.0);
    while t < len as f32 / SR {
        let gain = between(noise, 0.25, 0.45);
        crack(&mut out, noise, t, (0.004, 0.009), (700.0, 1400.0), gain);
        t += gap(noise, 0.45);
    }
    out
}

/// A log snapping in a fire: a sharp, bright crack with a woody knock under it, two to
/// five smaller cracks after as the split runs on, and a short breath of flame.
fn snap(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(0.6)];
    crack(&mut out, noise, 0.005, (0.002, 0.004), (2500.0, 4000.0), 1.0);
    crack(&mut out, noise, 0.006, (0.006, 0.012), (600.0, 1100.0), 0.6);
    let mut t = between(noise, 0.03, 0.08);
    for _ in 0..2 + (noise.unit() * 4.0) as usize {
        let gain = between(noise, 0.15, 0.45);
        crack(&mut out, noise, t, (0.0015, 0.004), (1800.0, 4500.0), gain);
        t += between(noise, 0.02, 0.1);
    }
    let mut flame = Svf::new(400.0, 0.7);
    let length = out.len() as f32;
    for (i, sample) in out.iter_mut().enumerate() {
        let x = i as f32 / length;
        *sample += 0.06 * flame.band(noise.next()) * (PI * x).sin().powi(2);
    }
    out
}

/// A tap of noise (`tap`) whose length and pitch are drawn from ranges.
fn crack(out: &mut [f32], noise: &mut Noise, at: f32, length: (f32, f32), center: (f32, f32), gain: f32) {
    let (length, center) = (between(noise, length.0, length.1), between(noise, center.0, center.1));
    tap(out, noise, at, length, center, gain);
}

/// A waterfall: pink noise roaring through a broad band, the low thunder of the plunge
/// below it and the hiss of spray above, swelling and easing a little.
fn waterfall(noise: &mut Noise, len: usize) -> Vec<f32> {
    let surge = Drift::new(noise, 0.4);
    let churn = Drift::new(noise, 7.0);
    let mut pink = Pink::default();
    let mut roar = Svf::new(900.0, 0.4);
    let mut plunge = Svf::new(110.0, 0.7);
    let mut spray = Svf::new(4500.0, 0.6);
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let p = pink.next(noise.next());
            let s = 0.85 + 0.15 * surge.unit(t) + 0.05 * churn.at(t);
            (roar.band(p) * 0.9 + plunge.low(p) * 0.6 + spray.band(noise.next()) * 0.05) * s
        })
        .collect()
}

// --- One-shots ---

/// Trills and twitters: a few quick notes, then a short motif repeated as a trill.
fn warbler(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let base = between(noise, 2600.0, 4200.0);
    let mut t = 0.03;
    let note = |noise: &mut Noise| {
        let from = base * between(noise, 0.75, 1.4);
        (from, from * between(noise, 0.7, 1.3), between(noise, 0.035, 0.08))
    };
    for _ in 0..2 + (noise.unit() * 4.0) as usize {
        let (from, to, length) = note(noise);
        tone(&mut out, t, length, from, to, between(noise, 0.5, 1.0), 0.2, &[0.12]);
        t += length + between(noise, 0.02, 0.06);
    }
    let motif: Vec<_> = (0..2 + (noise.unit() * 2.0) as usize).map(|_| note(noise)).collect();
    for _ in 0..2 + (noise.unit() * 4.0) as usize {
        for &(from, to, length) in &motif {
            tone(&mut out, t, length * 0.8, from, to, 0.8, 0.2, &[0.12]);
            t += length * 0.8 + 0.012;
        }
    }
    let (from, to, length) = note(noise);
    tone(&mut out, t + 0.04, length * 1.6, from, to * 0.85, 0.6, 0.15, &[0.12]);
    out
}

/// A thrush's song: a fluted phrase of a few notes, sung two or three times, then a
/// quieter twitter.
fn thrush(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let phrase: Vec<_> = (0..3 + (noise.unit() * 3.0) as usize)
        .map(|_| {
            let from = between(noise, 1600.0, 3800.0);
            (from, from * between(noise, 0.8, 1.2), between(noise, 0.09, 0.22))
        })
        .collect();
    let mut t = 0.03;
    for _ in 0..2 + (noise.unit() * 2.0) as usize {
        for &(from, to, length) in &phrase {
            tone(&mut out, t, length, from, to, 0.9, 0.25, &[0.15, 0.04]);
            t += length + 0.03;
        }
        t += between(noise, 0.12, 0.25);
    }
    for _ in 0..4 {
        let from = between(noise, 3000.0, 5000.0);
        tone(&mut out, t, 0.04, from, from * 1.2, 0.35, 0.2, &[0.1]);
        t += 0.06;
    }
    out
}

/// Two or three clear whistles, each lower than the last, the second with a slight hitch.
fn whistler(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let high = between(noise, 3700.0, 4200.0);
    tone(&mut out, 0.03, 0.32, high, high * 0.98, 1.0, 0.15, &[0.05]);
    let low = high * between(noise, 0.8, 0.86);
    tone(&mut out, 0.43, 0.13, low, low * 1.01, 0.9, 0.3, &[0.05]);
    tone(&mut out, 0.57, 0.15, low * 1.01, low * 0.99, 0.9, 0.2, &[0.05]);
    if noise.unit() < 0.4 {
        tone(&mut out, 0.82, 0.25, low * 0.98, low * 0.96, 0.7, 0.2, &[0.05]);
    }
    out
}

/// A dove's coo, "hoo-HOO-hoo-hoo": soft, low and rounded, each note rising then
/// falling.
fn dove(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let pitch = between(noise, 420.0, 520.0);
    let mut t = 0.05;
    for (length, gain) in [(0.35, 0.6), (0.55, 1.0), (0.3, 0.7), (0.3, 0.6)] {
        let half = length / 2.0;
        tone(&mut out, t, half, pitch, pitch * 1.1, gain, 0.4, &[0.3, 0.1]);
        tone(&mut out, t + half, half, pitch * 1.1, pitch * 0.9, gain, 0.05, &[0.3, 0.1]);
        t += length + between(noise, 0.12, 0.2);
    }
    let mut soft = Svf::new(1200.0, 0.7);
    out.iter_mut().for_each(|s| *s = soft.low(*s));
    out
}

/// A woodpecker's drum roll: a quick, even run of woody knocks, fading a little.
fn woodpecker(noise: &mut Noise) -> Vec<f32> {
    let knocks = 14 + (noise.unit() * 10.0) as usize;
    let interval = 1.0 / between(noise, 16.0, 22.0);
    let body = between(noise, 650.0, 900.0);
    let mut out = vec![0.0f32; seconds(knocks as f32 * interval + 0.2)];
    let mut band = Svf::new(1500.0, 2.0);
    for k in 0..knocks {
        let start = seconds(0.02 + k as f32 * interval);
        let gain = 0.95f32.powi(k as i32);
        for j in 0..seconds(0.04) {
            let local = j as f32 / SR;
            let crack = band.band(noise.next()) * (-local * 400.0).exp();
            out[start + j] += gain * (crack + 0.6 * (TAU * body * local).sin() * (-local * 90.0).exp());
        }
    }
    out
}

/// An owl's hoots, low and breathy: one, then a pair or a run.
fn owl(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let pitch = between(noise, 330.0, 390.0);
    let mut t = 0.05;
    for _ in 0..2 + (noise.unit() * 3.0) as usize {
        let length = between(noise, 0.25, 0.5);
        tone(&mut out, t, length, pitch * 1.02, pitch * 0.95, 1.0, 0.25, &[0.2, 0.05]);
        t += length + between(noise, 0.15, 0.6);
    }
    let mut breath = Svf::new(600.0, 0.8);
    let len = out.len();
    for (i, sample) in out.iter_mut().enumerate() {
        *sample += breath.band(noise.next()) * 0.08 * sample.abs().min(1.0) * (1.0 - i as f32 / len as f32);
    }
    out
}

/// One cricket chirping for a few seconds.
fn cricket(noise: &mut Noise) -> Vec<f32> {
    let frequency = between(noise, 4200.0, 5000.0);
    let period = between(noise, 0.4, 0.6);
    let mut out = vec![0.0f32; seconds(between(noise, 2.0, 3.5))];
    let mut t = 0.02;
    while seconds(t + 0.12) < out.len() {
        for p in 0..3 {
            let start = seconds(t + p as f32 * 0.03);
            let count = seconds(0.016);
            for j in 0..count {
                out[start + j] += (PI * j as f32 / count as f32).sin().powi(2) * (TAU * frequency * j as f32 / SR).sin();
            }
        }
        t += period;
    }
    out
}

/// A frog's croaks: buzzy pulse trains through the throat's resonances.
fn frog(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let pitch = between(noise, 90.0, 140.0);
    let formants = (between(noise, 600.0, 900.0), between(noise, 1300.0, 1700.0));
    let mut t = 0.03;
    for _ in 0..1 + (noise.unit() * 3.0) as usize {
        let length = between(noise, 0.2, 0.4);
        let flutter = between(noise, 14.0, 24.0);
        let (mut low, mut high) = (Svf::new(formants.0, 3.0), Svf::new(formants.1, 4.0));
        let first = seconds(t);
        let count = seconds(length);
        out.resize(first + count, 0.0);
        let mut phase = 0.0f32;
        for j in 0..count {
            let local = j as f32 / SR;
            phase = (phase + pitch * (1.0 - 0.1 * local / length) / SR).fract();
            let pulse = (-((phase - 0.5) / 0.06).powi(2)).exp() - 0.15;
            let throb = 0.6 + 0.4 * (TAU * flutter * local).sin();
            out[first + j] += (low.band(pulse) + 0.5 * high.band(pulse)) * throb * note_envelope(j as f32 / count as f32, 0.1);
        }
        t += length + between(noise, 0.25, 0.5);
    }
    out
}

/// Water gurgling past a stone nearby: the hush of a little splash, then a few small
/// bubbles in quick succession, each fainter than the last.
fn plop(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(0.45)];
    let mut hush = Svf::new(between(noise, 1500.0, 2500.0), 0.7);
    let mut pink = Pink::default();
    for (j, sample) in out.iter_mut().enumerate().take(seconds(0.12)) {
        let t = j as f32 / SR;
        *sample += hush.band(pink.next(noise.next())) * 0.5 * (t / 0.004).min(1.0) * (-t / 0.03).exp();
    }
    let mut t = between(noise, 0.01, 0.03);
    let mut gain = 1.0;
    for _ in 0..3 + (noise.unit() * 4.0) as usize {
        bubble(&mut out, t, between(noise, 700.0, 2200.0), gain * between(noise, 0.5, 1.0), between(noise, 0.1, 0.35));
        t += between(noise, 0.02, 0.07);
        gain *= 0.75;
    }
    out
}

/// A bubble of lava bursting: a low, thick blurp, a second smaller one, and a spatter of
/// drops landing around.
fn lava_pop(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(0.8)];
    let frequency = between(noise, 60.0, 130.0);
    bubble(&mut out, 0.01, frequency, 1.0, between(noise, 0.5, 0.9));
    bubble(&mut out, between(noise, 0.08, 0.15), frequency * between(noise, 1.3, 1.8), 0.4, 0.6);
    // Spatter: a few soft taps after the burst.
    let mut t = between(noise, 0.06, 0.12);
    for _ in 0..3 + (noise.unit() * 5.0) as usize {
        let mut band = Svf::new(between(noise, 1200.0, 3000.0), 0.9);
        let gain = between(noise, 0.05, 0.2);
        let length = between(noise, 0.004, 0.012);
        add_at(&mut out, t, seconds(length * 5.0), |j| gain * band.band(noise.next()) * (-(j as f32 / SR) / length).exp());
        t += between(noise, 0.02, 0.09);
    }
    out
}

/// A pot simmering: bubbles smaller and higher than lava's, in a quick, uneven run, the
/// first ones the fullest.
fn simmer(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(0.9)];
    let mut t = between(noise, 0.01, 0.03);
    let mut gain = 1.0;
    for _ in 0..5 + (noise.unit() * 5.0) as usize {
        bubble(&mut out, t, between(noise, 260.0, 620.0), gain * between(noise, 0.5, 1.0), between(noise, 0.3, 0.6));
        t += between(noise, 0.03, 0.09);
        gain *= 0.85;
    }
    out
}

/// A gust: air rising in pitch and strength over a second or two, then passing.
fn gust(noise: &mut Noise) -> Vec<f32> {
    let length = seconds(between(noise, 2.5, 4.0));
    let peak = between(noise, 0.35, 0.55);
    let mut body = Svf::new(300.0, 0.8);
    let mut air = Svf::new(2500.0, 0.6);
    (0..length)
        .map(|i| {
            let x = i as f32 / length as f32;
            let swell = if x < peak { (PI / 2.0 * x / peak).sin().powi(2) } else { (PI / 2.0 * (1.0 - x) / (1.0 - peak)).sin().powi(2) };
            let white = noise.next();
            body.tune(250.0 + 900.0 * swell);
            body.band(white) * swell + air.band(white) * 0.2 * swell * swell
        })
        .collect()
}

/// A drop landing in a pool: a click, then a bright tone that bends upwards as it rings.
fn drip(noise: &mut Noise) -> Vec<f32> {
    let pitch = between(noise, 1400.0, 3200.0);
    let decay = between(noise, 30.0, 45.0);
    let mut osc = Osc::default();
    (0..seconds(0.25))
        .map(|i| {
            let t = i as f32 / SR;
            let click = if t < 0.001 { noise.next() * 0.3 } else { 0.0 };
            click + osc.next(pitch * (1.0 + 0.25 * (1.0 - (-t * 60.0).exp()))) * (-t * decay).exp()
        })
        .collect()
}

/// Rock settling far off: a long, very low swell with a little grit.
fn rumble(noise: &mut Noise) -> Vec<f32> {
    let length = seconds(between(noise, 4.0, 6.0));
    let rise = between(noise, 0.15, 0.35);
    let (mut a, mut b, mut grit) = (Svf::new(60.0, 0.7), Svf::new(60.0, 0.7), Svf::new(130.0, 2.0));
    (0..length)
        .map(|i| {
            let x = i as f32 / length as f32;
            let white = noise.next();
            let swell = if x < rise { (x / rise).powi(2) } else { ((1.0 - x) / (1.0 - rise)).powf(1.5) };
            (b.low(a.low(white)) * 4.0 + grit.band(white) * 0.3) * swell
        })
        .collect()
}

/// Grit loosed in a dry cave: a handful of tiny bright ticks, quickening and thinning as the
/// stones run down the rock, over a soft hiss of dust that settles after them.
fn grit(noise: &mut Noise) -> Vec<f32> {
    let length = seconds(between(noise, 0.6, 1.0));
    let ticks: Vec<(usize, f32)> = (0..between(noise, 5.0, 9.0) as usize)
        .map(|_| {
            let at = between(noise, 0.0, 1.0).powf(1.6) * 0.7;
            ((at * length as f32) as usize, between(noise, 0.3, 1.0))
        })
        .collect();
    let (mut tick, mut dust) = (Svf::new(4200.0, 3.0), Svf::new(2600.0, 0.8));
    (0..length)
        .map(|i| {
            let x = i as f32 / length as f32;
            let white = noise.next();
            let hit: f32 = ticks.iter().filter(|(at, _)| i >= *at && i < at + 90).map(|(at, gain)| gain * (-((i - at) as f32) / 18.0).exp()).sum();
            tick.band(white * hit) * 1.6 + dust.band(white) * 0.06 * (x * 4.0).min(1.0) * (1.0 - x).powi(2)
        })
        .collect()
}

/// Wood straining: stick-slip pulses whose rate glides, through the wood's resonances.
fn creak(noise: &mut Noise) -> Vec<f32> {
    let length = seconds(between(noise, 0.5, 1.1));
    let (from, to) = if noise.unit() < 0.5 { (70.0, 140.0) } else { (140.0, 80.0) };
    let mut out = vec![0.0f32; length];
    let mut t = 0.0f32;
    while seconds(t) < length {
        let x = seconds(t) as f32 / length as f32;
        let start = seconds(t);
        for (j, sample) in out[start..].iter_mut().enumerate().take(seconds(0.002)) {
            *sample += (-(j as f32) / SR * 3000.0).exp() * (0.6 + 0.4 * noise.unit());
        }
        t += (1.0 / (from + (to - from) * x)) * (0.9 + 0.2 * noise.unit());
    }
    let (mut low, mut high) = (Svf::new(between(noise, 700.0, 1000.0), 4.0), Svf::new(2200.0, 5.0));
    for (i, sample) in out.iter_mut().enumerate() {
        let x = *sample;
        *sample = (low.band(x) + 0.4 * high.band(x)) * note_envelope(i as f32 / length as f32, 0.3);
    }
    out
}

// --- A settlement at work ---

/// Decaying sine partials (`(ratio, gain, decay seconds)` of `frequency`) added at `t`:
/// something struck ringing.
fn ring(out: &mut [f32], t: f32, frequency: f32, partials: &[(f32, f32, f32)], gain: f32) {
    let longest = partials.iter().fold(0.0f32, |l, p| l.max(p.2));
    add_at(out, t, seconds(longest * 6.0), |j| {
        let local = j as f32 / SR;
        let onset = (local / 0.0006).min(1.0);
        gain * onset * partials.iter().map(|&(ratio, level, decay)| level * (TAU * frequency * ratio * local).sin() * (-local / decay).exp()).sum::<f32>()
    });
}

fn hammer(noise: &mut Noise) -> Vec<f32> {
    let blows = 3 + (noise.unit() * 4.0) as usize;
    let every = between(noise, 0.42, 0.6);
    let mut out = vec![0.0f32; seconds(blows as f32 * every + 0.5)];
    let (board, nail) = (between(noise, 180.0, 260.0), between(noise, 2400.0, 3400.0));
    let mut t = 0.03;
    for blow in 0..blows {
        let gain = if blow + 1 == blows { 1.1 } else { between(noise, 0.75, 1.0) };
        crack(&mut out, noise, t, (0.002, 0.004), (2000.0, 3500.0), 0.8 * gain);
        ring(&mut out, t, board * between(noise, 0.95, 1.05), &[(1.0, 1.0, 0.03), (2.3, 0.5, 0.018), (4.1, 0.2, 0.008)], 0.7 * gain);
        // The nail rings while it still stands proud; the last blows sink it.
        if blow + 2 < blows && noise.unit() < 0.7 {
            ring(&mut out, t, nail, &[(1.0, 1.0, 0.05), (2.76, 0.3, 0.02)], 0.18 * gain);
        }
        t += every * between(noise, 0.9, 1.1);
    }
    out
}

fn saw(noise: &mut Noise) -> Vec<f32> {
    let strokes = 4 + (noise.unit() * 5.0) as usize;
    let length = between(noise, 0.3, 0.45);
    let teeth = between(noise, 70.0, 110.0);
    let mut out = vec![0.0f32; seconds(strokes as f32 * length + 0.3)];
    let mut band = Svf::new(between(noise, 2600.0, 3600.0), 1.4);
    let mut body = Svf::new(between(noise, 600.0, 900.0), 2.0);
    let mut phase = 0.0f32;
    for stroke in 0..strokes {
        // The push bites; the pull is lighter and a little shorter.
        let push = stroke % 2 == 0;
        let (gain, span) = if push { (1.0, length) } else { (0.55, length * 0.85) };
        let first = seconds(0.03 + stroke as f32 * length);
        let count = seconds(span);
        for j in 0..count {
            let x = j as f32 / count as f32;
            let speed = (PI * x).sin();
            phase = (phase + teeth * (0.4 + speed) / SR).fract();
            let rasp = 0.35 + 0.65 * (1.0 - phase).powi(3);
            let grain = noise.next() * rasp;
            if let Some(sample) = out.get_mut(first + j) {
                *sample += gain * note_envelope(x, 0.15) * (band.band(grain) + 0.5 * body.band(grain));
            }
        }
    }
    out
}

fn hen(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let pitch = between(noise, 380.0, 520.0);
    let mut t = 0.03;
    for _ in 0..3 + (noise.unit() * 4.0) as usize {
        let length = between(noise, 0.05, 0.09);
        tone(&mut out, t, length, pitch * 1.15, pitch * 0.9, between(noise, 0.6, 1.0), 0.15, &[0.7, 0.5, 0.35, 0.2]);
        t += length + between(noise, 0.08, 0.3);
    }
    // Now and then the long call: a short note, then one rising and falling.
    if noise.unit() < 0.4 {
        t += 0.1;
        tone(&mut out, t, 0.08, pitch * 1.1, pitch, 0.8, 0.15, &[0.7, 0.5, 0.35, 0.2]);
        tone(&mut out, t + 0.13, 0.35, pitch * 1.3, pitch * 1.05, 1.0, 0.2, &[0.8, 0.6, 0.4, 0.25]);
    }
    // A nasal throat: the clucks rung through two narrow bands.
    let (mut nose, mut throat) = (Svf::new(between(noise, 1100.0, 1400.0), 3.0), Svf::new(between(noise, 2400.0, 2900.0), 4.0));
    out.iter_mut().for_each(|s| *s = nose.band(*s) + 0.6 * throat.band(*s) + 0.15 * *s);
    out
}

fn well(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(2.2)];
    // The winch: slow ticks of a ratchet and a creak on its axle.
    let mut t = 0.03;
    let ticks = 4 + (noise.unit() * 4.0) as usize;
    for _ in 0..ticks {
        crack(&mut out, noise, t, (0.002, 0.004), (1200.0, 1800.0), 0.35);
        t += between(noise, 0.09, 0.14);
    }
    let mut axle = Svf::new(between(noise, 600.0, 900.0), 5.0);
    let (start, length) = (seconds(0.02), seconds(t));
    let mut pulse = 0.0f32;
    for j in 0..length {
        pulse = (pulse + between(noise, 90.0, 120.0) / SR).fract();
        let click = if pulse < 0.05 { noise.next() } else { 0.0 };
        out[start + j] += 0.5 * axle.band(click) * note_envelope(j as f32 / length as f32, 0.3);
    }
    // The bucket comes up: water slops over its lip, then it knocks against the rim.
    t += 0.08;
    let mut slosh = Svf::new(between(noise, 700.0, 1100.0), 1.0);
    let span = seconds(0.45);
    let first = seconds(t);
    for j in 0..span {
        let x = j as f32 / span as f32;
        out[first + j] += 0.5 * slosh.band(noise.next()) * note_envelope(x, 0.1) * (0.6 + 0.4 * (TAU * 3.0 * x).sin());
    }
    for _ in 0..4 + (noise.unit() * 4.0) as usize {
        bubble(&mut out, t + between(noise, 0.0, 0.4), between(noise, 500.0, 1300.0), between(noise, 0.15, 0.35), 0.4);
    }
    let knock = t + between(noise, 0.5, 0.65);
    ring(&mut out, knock, between(noise, 200.0, 260.0), &[(1.0, 1.0, 0.04), (2.4, 0.5, 0.02), (5.1, 0.2, 0.008)], 0.9);
    crack(&mut out, noise, knock, (0.002, 0.004), (1500.0, 2500.0), 0.6);
    out
}

/// How many samples the hum's slow changes hold for (about 1.3 ms).
const CONTROL: usize = 64;

/// Bees about their hive: a score of wings, each a buzz near 230 Hz (a wingbeat and its
/// overtones) wandering in pitch as the bee turns, and in loudness as it comes and goes, so
/// the drone breathes; now and then one passes close, louder and bending down as it goes
/// by. Kept under two kilohertz, as wings are heard through the air.
fn hum(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    let total = len as f32 / SR;
    let bee = |out: &mut [f32], noise: &mut Noise, start: f32, length: f32, pitch: f32, level: f32, passing: bool| {
        let (turning, nearing) = (between(noise, 0.4, 1.2), between(noise, 0.1, 0.4));
        let (turn, near) = (Drift::new(noise, turning), Drift::new(noise, nearing));
        let mut phase = TAU * noise.unit();
        let (from, count) = (seconds(start.max(0.0)), seconds(length));
        let (mut step, mut gain) = (0.0, 0.0);
        for j in 0..count.min(len.saturating_sub(from)) {
            // The turn and the coming and going are slow: worked out every few
            // milliseconds, not every sample.
            if j % CONTROL == 0 {
                let (t, k) = ((from + j) as f32 / SR, j as f32 / count as f32);
                // A bee going by bends down past the listener, its loudness swelling and gone.
                let (bend, swell) = if passing { (1.04 - 0.08 * k, (PI * k).sin().powi(2)) } else { (1.0, 0.3 + 0.7 * near.unit(t)) };
                (step, gain) = (TAU * pitch * bend * (0.97 + 0.06 * turn.unit(t)) / SR, level * swell);
            }
            phase = (phase + step) % TAU;
            // The wingbeat and three overtones from one sine and cosine.
            let (sin, cos) = phase.sin_cos();
            let (sin2, cos2) = (2.0 * sin * cos, 1.0 - 2.0 * sin * sin);
            let wing = sin + 0.5 * sin2 + 0.25 * sin * (3.0 - 4.0 * sin * sin) + 0.12 * 2.0 * sin2 * cos2;
            out[from + j] += gain * wing;
        }
    };
    for _ in 0..20 {
        let (pitch, level) = (between(noise, 200.0, 260.0), between(noise, 0.025, 0.07));
        bee(&mut out, noise, 0.0, total, pitch, level, false);
    }
    let mut t = between(noise, 0.5, 3.0);
    while t < total {
        let (length, pitch, level) = (between(noise, 0.8, 1.6), between(noise, 210.0, 250.0), between(noise, 0.15, 0.25));
        bee(&mut out, noise, t, length, pitch, level, true);
        t += length + gap(noise, 0.25);
    }
    let mut air = Svf::new(1800.0, 0.7);
    out.iter_mut().for_each(|s| *s = air.low(*s));
    out
}

/// Many voices across a settlement: each a talker whose syllables come four to six a
/// second in phrases with pauses between, a breath of noise through two vowel bands that
/// wander as the words change; far off, so low-passed into a blur, over a soft hush. Now
/// and then one rises over the rest, a laugh or a call.
fn murmur(noise: &mut Noise, len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    let total = len as f32 / SR;
    for talker in 0..7 {
        let (low, high) = (between(noise, 380.0, 700.0), between(noise, 1100.0, 1900.0));
        let (vowel_a, vowel_b) = (Drift::new(noise, 3.0), Drift::new(noise, 2.4));
        let (mut a, mut b) = (Svf::new(low, 4.0), Svf::new(high, 5.0));
        let level = between(noise, 0.5, 1.0) * if talker == 0 { 1.4 } else { 1.0 };
        let mut t = between(noise, 0.0, 2.0);
        while t < total {
            // A phrase: a run of syllables, then a pause.
            let syllables = 3 + (noise.unit() * 10.0) as usize;
            let rate = between(noise, 4.0, 6.0);
            let lift = if noise.unit() < 0.08 { 2.0 } else { 1.0 };
            for _ in 0..syllables {
                let length = seconds(between(noise, 0.6, 1.0) / rate);
                let start = seconds(t);
                for j in 0..length.min(len.saturating_sub(start)) {
                    let at = (start + j) as f32 / SR;
                    a.tune(low * (0.8 + 0.4 * vowel_a.unit(at)));
                    b.tune(high * (0.8 + 0.4 * vowel_b.unit(at)));
                    let white = noise.next();
                    out[start + j] += level * lift * (a.band(white) + 0.5 * b.band(white)) * note_envelope(j as f32 / length as f32, 0.3);
                }
                t += 1.0 / rate;
            }
            t += gap(noise, 0.6);
        }
    }
    // Far off: the highs gone, and a soft hush of the place under the voices.
    let mut far = Svf::new(900.0, 0.6);
    let mut hush = Svf::new(500.0, 0.5);
    out.iter_mut().for_each(|s| *s = far.low(*s) * 0.6 + hush.band(noise.next()) * 0.03);
    out
}

/// A door: the latch lifted with a click and a rattle, a short creak as it swings, and the
/// door knocking home in its frame, a low wooden thump and the latch dropping.
fn door(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(1.6)];
    crack(&mut out, noise, 0.02, (0.002, 0.004), (2200.0, 3200.0), 0.5);
    crack(&mut out, noise, 0.05, (0.002, 0.003), (1800.0, 2600.0), 0.3);
    // The hinge: stick-slip pulses rung through the board.
    let (start, length) = (0.12, between(noise, 0.25, 0.45));
    let mut hinge = Svf::new(between(noise, 650.0, 950.0), 5.0);
    let rate = between(noise, 90.0, 150.0);
    let count = seconds(length);
    let mut pulse = 0.0f32;
    let first = seconds(start);
    for j in 0..count {
        pulse = (pulse + rate * (0.9 + 0.2 * noise.unit()) / SR).fract();
        let click = if pulse < 0.04 { noise.next() } else { 0.0 };
        out[first + j] += 0.5 * hinge.band(click) * note_envelope(j as f32 / count as f32, 0.3);
    }
    let shut = start + length + between(noise, 0.15, 0.4);
    let board = between(noise, 95.0, 130.0);
    ring(&mut out, shut, board, &[(1.0, 1.0, 0.06), (2.1, 0.5, 0.03), (3.7, 0.2, 0.015)], 0.9);
    let mut thump = Svf::new(220.0, 0.7);
    let first = seconds(shut);
    for j in 0..seconds(0.08) {
        out[first + j] += thump.low(noise.next()) * (-(j as f32) / SR / 0.02).exp() * 0.8;
    }
    crack(&mut out, noise, shut + 0.04, (0.002, 0.004), (2000.0, 3000.0), 0.35);
    out
}

/// A dog barking once, twice or thrice: each a short, rough, falling cry through the
/// mouth's two bands, a little apart.
fn bark(noise: &mut Noise) -> Vec<f32> {
    let mut out = Vec::new();
    let pitch = between(noise, 330.0, 520.0);
    let mut t = 0.02;
    for _ in 0..1 + (noise.unit() * 3.0) as usize {
        let length = between(noise, 0.09, 0.16);
        tone(&mut out, t, length, pitch * 1.25, pitch * 0.8, between(noise, 0.8, 1.0), 0.08, &[0.9, 0.7, 0.55, 0.4, 0.3]);
        let (begin, rough) = (seconds(t), seconds(length));
        for j in 0..rough.min(out.len().saturating_sub(begin)) {
            out[begin + j] += 0.25 * noise.next() * note_envelope(j as f32 / rough as f32, 0.08);
        }
        t += length + between(noise, 0.18, 0.4);
    }
    let (mut mouth, mut throat) = (Svf::new(between(noise, 750.0, 1000.0), 2.5), Svf::new(between(noise, 1700.0, 2200.0), 3.0));
    out.iter_mut().for_each(|s| *s = mouth.band(*s) + 0.5 * throat.band(*s) + 0.1 * *s);
    out
}

/// A sheep's bleat: a nasal, wavering cry, the voice trembling on it, falling at the end.
fn bleat(noise: &mut Noise) -> Vec<f32> {
    let length = between(noise, 0.5, 0.9);
    let pitch = between(noise, 230.0, 320.0);
    let tremble = between(noise, 6.0, 9.0);
    let count = seconds(length);
    let mut out = vec![0.0f32; count + seconds(0.05)];
    let mut phase = 0.0f32;
    for (i, sample) in out.iter_mut().enumerate().take(count) {
        let x = i as f32 / count as f32;
        let wobble = (TAU * tremble * i as f32 / SR).sin();
        let f = pitch * (1.0 + 0.04 * wobble) * (1.0 - 0.12 * x * x);
        phase = (phase + f / SR).fract();
        let wave: f32 = (1..9).map(|k| (TAU * phase * k as f32).sin() / k as f32).sum();
        *sample = wave * (0.75 + 0.25 * wobble) * note_envelope(x, 0.15);
    }
    let (mut nose, mut mouth) = (Svf::new(between(noise, 900.0, 1200.0), 3.0), Svf::new(between(noise, 2300.0, 2800.0), 4.0));
    out.iter_mut().for_each(|s| *s = nose.band(*s) + 0.5 * mouth.band(*s) + 0.1 * *s);
    out
}

/// The smith at the anvil: two to four rounds of a heavy blow on the iron and one or two
/// light taps on the anvil's face between, the smith's rhythm; each a sharp strike and a
/// bright, inharmonic ring (a bar's partials) that carries far.
fn anvil(noise: &mut Noise) -> Vec<f32> {
    let rounds = 2 + (noise.unit() * 3.0) as usize;
    let every = between(noise, 0.55, 0.75);
    let mut out = vec![0.0f32; seconds(rounds as f32 * every + 2.4)];
    let iron = between(noise, 950.0, 1250.0);
    let bar = [(1.0, 1.0, 0.3), (2.76, 0.6, 0.16), (5.4, 0.35, 0.08), (8.93, 0.2, 0.04)];
    let mut t = 0.02;
    for _ in 0..rounds {
        let gain = between(noise, 0.8, 1.0);
        crack(&mut out, noise, t, (0.001, 0.003), (3000.0, 5000.0), 0.6 * gain);
        ring(&mut out, t, iron * between(noise, 0.98, 1.02), &bar, 0.7 * gain);
        let taps = 1 + (noise.unit() * 2.0) as usize;
        for tap in 0..taps {
            let at = t + every * (tap + 1) as f32 / (taps + 1) as f32;
            ring(&mut out, at, iron * 1.12, &[(1.0, 1.0, 0.12), (2.76, 0.4, 0.06)], 0.25);
        }
        t += every * between(noise, 0.95, 1.05);
    }
    out
}

/// A small shrine bell rung two or three times: a struck bell's partials (a hum an
/// octave down, the strike note, a minor third, a fifth, the octave and a high ring),
/// the high ones dying first so each stroke mellows into its hum.
fn bell(noise: &mut Noise) -> Vec<f32> {
    let strokes = 2 + (noise.unit() * 2.0) as usize;
    let every = between(noise, 1.2, 1.4);
    let strike = between(noise, 560.0, 700.0);
    let partials = [(0.5, 0.6, 0.75), (1.0, 1.0, 0.55), (1.19, 0.5, 0.4), (1.5, 0.35, 0.3), (2.0, 0.4, 0.22), (2.74, 0.2, 0.12)];
    let mut out = vec![0.0f32; seconds((strokes - 1) as f32 * every + 0.75 * 6.0 + 0.1)];
    for stroke in 0..strokes {
        let at = 0.02 + stroke as f32 * every;
        crack(&mut out, noise, at, (0.001, 0.002), (2500.0, 3500.0), 0.15);
        ring(&mut out, at, strike, &partials, between(noise, 0.75, 1.0));
    }
    out
}

/// A mill wheel turning: a paddle knocking into the race every half second or so, deep
/// and wooden, the axle groaning once round, and water spilling off the paddles between.
fn mill(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(4.0)];
    let every = between(noise, 0.45, 0.6);
    let board = between(noise, 110.0, 150.0);
    let mut t = 0.03;
    while t < 3.3 {
        ring(&mut out, t, board * between(noise, 0.96, 1.04), &[(1.0, 1.0, 0.05), (2.3, 0.4, 0.025)], between(noise, 0.5, 0.8));
        crack(&mut out, noise, t, (0.003, 0.006), (500.0, 900.0), 0.3);
        t += every;
    }
    // The axle: a slow groan once round.
    let groan = creak(noise);
    let at = seconds(between(noise, 0.5, 1.5));
    for (sample, g) in out.iter_mut().skip(at).zip(groan) {
        *sample += 0.5 * g;
    }
    // Water spilling: soft bright splashes off each paddle as it rises.
    let mut spill = Svf::new(1600.0, 0.8);
    let total = out.len();
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let pour = (PI * t / every).sin().abs().powi(4) * note_envelope(i as f32 / total as f32, 0.1);
        *sample += 0.25 * pour * spill.band(noise.next());
    }
    out
}

/// Wood split at a woodyard: an axe biting into a log, a bright crack over a dull thunk,
/// two to four times; the last splits it, the halves knocking down onto the block.
fn chop(noise: &mut Noise) -> Vec<f32> {
    let strokes = 2 + (noise.unit() * 3.0) as usize;
    let every = between(noise, 0.9, 1.3);
    let mut out = vec![0.0f32; seconds(strokes as f32 * every + 0.8)];
    let log = between(noise, 280.0, 400.0);
    let mut t = 0.02;
    for _ in 0..strokes {
        crack(&mut out, noise, t, (0.002, 0.004), (2500.0, 4000.0), 0.8);
        ring(&mut out, t, log * between(noise, 0.95, 1.05), &[(1.0, 1.0, 0.025), (2.1, 0.4, 0.012)], 0.8);
        crack(&mut out, noise, t + 0.002, (0.008, 0.015), (300.0, 600.0), 0.5);
        t += every * between(noise, 0.95, 1.05);
    }
    // The split: a sharper crack, and the halves falling.
    let split = t - every;
    crack(&mut out, noise, split + 0.01, (0.003, 0.006), (1500.0, 2500.0), 0.6);
    for (after, gain) in [(0.3, 0.5), (0.42, 0.35)] {
        ring(&mut out, split + after + between(noise, 0.0, 0.05), log * 0.7, &[(1.0, 1.0, 0.02), (2.4, 0.3, 0.01)], gain);
    }
    out
}

/// A log settling in the fire: a soft, low fall into the embers, a burst of sparks
/// crackling up, and a breath of hiss as the wood finds the heat.
fn settle(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(1.6)];
    ring(&mut out, 0.02, between(noise, 90.0, 130.0), &[(1.0, 1.0, 0.04), (2.2, 0.3, 0.02)], 0.6);
    crack(&mut out, noise, 0.02, (0.01, 0.02), (300.0, 600.0), 0.4);
    let mut t = 0.05;
    for _ in 0..8 + (noise.unit() * 8.0) as usize {
        let gain = between(noise, 0.08, 0.3);
        crack(&mut out, noise, t, (0.001, 0.003), (2000.0, 5000.0), gain);
        t += between(noise, 0.01, 0.06);
    }
    let mut hiss = Svf::new(3200.0, 0.7);
    let count = seconds(1.2);
    for (j, sample) in out.iter_mut().skip(seconds(0.1)).take(count).enumerate() {
        *sample += 0.12 * note_envelope(j as f32 / count as f32, 0.15) * hiss.band(noise.next());
    }
    out
}

/// A pig rooting in its pen: two to four short, low grunts, rough and nasal, and now
/// and then a snort.
fn grunt(noise: &mut Noise) -> Vec<f32> {
    let grunts = 2 + (noise.unit() * 3.0) as usize;
    let mut out = vec![0.0f32; seconds(grunts as f32 * 0.55 + 0.4)];
    let mut t = 0.02;
    for _ in 0..grunts {
        let (length, pitch) = (between(noise, 0.12, 0.25), between(noise, 85.0, 130.0));
        let count = seconds(length);
        let (start, mut phase) = (seconds(t), 0.0f32);
        let mut nose = Svf::new(between(noise, 500.0, 750.0), 2.5);
        for j in 0..count {
            let x = j as f32 / count as f32;
            phase = (phase + pitch * (1.0 - 0.15 * x) / SR).fract();
            // A buzzing pulse, roughened with breath.
            let pulse = (1.0 - phase).powi(6) - 0.15 + 0.3 * noise.next();
            if let Some(sample) = out.get_mut(start + j) {
                *sample += note_envelope(x, 0.1) * (nose.band(pulse) + 0.2 * pulse);
            }
        }
        t += length + between(noise, 0.12, 0.3);
    }
    if noise.unit() < 0.4 {
        let mut snout = Svf::new(900.0, 1.2);
        let count = seconds(0.15);
        let start = seconds(t.min(out.len() as f32 / SR - 0.2));
        for j in 0..count {
            out[start + j] += 0.5 * note_envelope(j as f32 / count as f32, 0.05) * snout.band(noise.next());
        }
    }
    out
}

/// Supper things: two to four knocks of a ladle on an iron pot and its lid, ringing
/// short, and a wooden bowl set down.
fn clatter(noise: &mut Noise) -> Vec<f32> {
    let mut out = vec![0.0f32; seconds(1.6)];
    let mut t = 0.02;
    for _ in 0..2 + (noise.unit() * 3.0) as usize {
        let iron = between(noise, 700.0, 1300.0);
        let gain = between(noise, 0.4, 0.8);
        ring(&mut out, t, iron, &[(1.0, 1.0, 0.09), (2.4, 0.5, 0.05), (3.9, 0.3, 0.03), (5.6, 0.15, 0.02)], gain);
        crack(&mut out, noise, t, (0.001, 0.003), (2500.0, 4000.0), 0.3);
        t += between(noise, 0.12, 0.3);
    }
    let bowl = between(noise, 300.0, 450.0);
    ring(&mut out, t + 0.1, bowl, &[(1.0, 1.0, 0.035), (2.6, 0.4, 0.015)], 0.6);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(samples: &[f32], what: &str) {
        assert!(!samples.is_empty(), "{what} is empty");
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0), "{what} has bad samples");
        assert!(loudness(samples, 1.0, 1e9).is_finite(), "{what}");
        let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
        assert!(power > 1e-5, "{what} is silent");
    }

    #[test]
    fn beds_are_clean_and_join_without_a_seam() {
        for synth in Synth::ALL.into_iter().filter(|s| s.is_bed()) {
            let [left, right] = bed(synth, 0);
            clean(&left, &format!("{synth:?} left"));
            clean(&right, &format!("{synth:?} right"));
            // Segments start and end silent and are full in the middle, so overlapped
            // segments crossfade evenly.
            assert!(left[0] == 0.0 && left.last().unwrap().abs() < 1e-3, "{synth:?}");
            let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
            let middle = rms(&left[left.len() / 3..2 * left.len() / 3]);
            assert!((0.05..0.4).contains(&middle), "{synth:?} middle loudness {middle}");
            // The channels differ, so the bed is wide.
            assert!(left.iter().zip(&right).any(|(l, r)| (l - r).abs() > 0.01), "{synth:?}");
        }
    }

    #[test]
    fn one_shots_are_clean_and_ring_on_in_a_cave() {
        for synth in Synth::ALL.into_iter().filter(|s| !s.is_bed()) {
            for variant in 0..2 {
                let (dry, [left, right]) = one_shot(synth, variant);
                clean(&dry, &format!("{synth:?}"));
                assert!(dry.len() < seconds(8.0), "{synth:?} is too long");
                assert!(dry.last().unwrap().abs() < 0.01, "{synth:?} ends abruptly");
                clean(&left, &format!("{synth:?} reverb"));
                assert!(left.len() > dry.len() && right.len() == left.len());
            }
        }
    }

    #[test]
    fn variants_differ() {
        assert_ne!(one_shot(Synth::Warbler, 0).0, one_shot(Synth::Warbler, 1).0);
        assert_ne!(bed(Synth::Wind, 0)[0], bed(Synth::Wind, 1)[0]);
    }
}
