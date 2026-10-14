//! How creatures move on screen, from `creature_motion.ron`, read and checked at launch:
//! each movement style's walk and run (how far the legs swing, how long a foot stays on the
//! ground, how the body bobs and rocks), how a body turns, what it does standing about, and
//! how a fleeing one looks. A species may give its own gaits. None of it changes where the
//! server puts a creature; it is how the body is drawn getting there.
//!
//! A foot on the ground never slides: the stride is worked out from the legs' length and
//! swing, so the stride's length is how far the body went while a foot was down
//! ([`stride_length`]), and through the part of the stride a foot is down it moves back
//! under the body at the body's own speed ([`leg`]).

use std::collections::BTreeMap;

use bevy::prelude::Resource;
use serde::Deserialize;
use voxmar_engine::content::ContentFile;
use voxmar_engine::creature::Style;

/// The file a package says how creatures move in.
pub const FILE: &str = "creature_motion.ron";

/// The movement styles, by the names the file gives them.
const STYLES: [(Style, &str); 8] = [
    (Style::Walker, "Walker"),
    (Style::Hopper, "Hopper"),
    (Style::Stalker, "Stalker"),
    (Style::Trotter, "Trotter"),
    (Style::Lumberer, "Lumberer"),
    (Style::Creeper, "Creeper"),
    (Style::Drifter, "Drifter"),
    (Style::Flier, "Flier"),
];

/// One gait: how far each leg swings either way from straight down at its fullest
/// (radians), the share of a stride each foot is on the ground, how much the body rises
/// and falls (of its height) and rocks nose to tail (radians), and how far a foot lifts
/// as it swings forward (a share of the leg drawn up).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gait {
    pub swing: f32,
    pub duty: f32,
    pub bob: f32,
    #[serde(default)]
    pub pitch: f32,
    pub lift: f32,
}

impl Gait {
    fn mix(self, other: Gait, t: f32) -> Gait {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Gait { swing: m(self.swing, other.swing), duty: m(self.duty, other.duty), bob: m(self.bob, other.bob), pitch: m(self.pitch, other.pitch), lift: m(self.lift, other.lift) }
    }
}

/// A style's (or a species') walk and run, and which of them move each pair of legs
/// together (a hop, a bound) rather than in turn.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gaits {
    pub walk: Gait,
    pub run: Gait,
    #[serde(default)]
    pub paired_walk: bool,
    #[serde(default)]
    pub paired_run: bool,
}

/// How a body turns: up to `share` of its species' turning speed, easing in over the last
/// `ease` seconds of a turn; leaning into a turn by `lean` (radians, for each radian a
/// second at a block a second), its head ahead of the turn by `head_lead` of what is left
/// of it, and stepping round when it turns on the spot as if it walked `step` of its length
/// for each radian.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub share: f32,
    pub ease: f32,
    pub lean: f32,
    pub lean_most: f32,
    pub head_lead: f32,
    pub step: f32,
}

/// Something done now and then while standing about: tried every `every` seconds, with
/// `chance`, lasting `hold` seconds, by `size` (radians).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Now {
    pub every: f32,
    pub chance: f32,
    pub hold: f32,
    pub size: f32,
}

/// Standing about: glancing aside, sniffing the ground, shifting its weight side to side
/// (radians of roll), and a flick of the tail.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Idle {
    pub glance: Now,
    pub sniff: Now,
    pub shift: f32,
    pub flick: Now,
}

/// A creature running away: its head pushed out (radians down), ears laid back, tail up
/// (radians), body stretched long (a share), legs reaching further (times).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flee {
    pub head: f32,
    pub ears: f32,
    pub tail: f32,
    pub stretch: f32,
    pub reach: f32,
}

/// A boss's (or any creature's) wind-up as its body shows it beyond its stance: a heavy
/// blow trembles (a share of its height), and a crawler raises its front legs by its
/// stance's `paw`.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tells {
    pub heavy_tremble: f32,
    pub crawler_front: f32,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionFile {
    #[serde(default)]
    gaits: BTreeMap<String, Gaits>,
    #[serde(default)]
    species: BTreeMap<String, Gaits>,
    turn: Option<Turn>,
    idle: Option<Idle>,
    flee: Option<Flee>,
    tells: Option<Tells>,
}

/// How creatures move on screen, every package's word merged.
#[derive(Resource, Clone, Debug)]
pub struct Motion {
    gaits: BTreeMap<String, Gaits>,
    species: BTreeMap<String, Gaits>,
    pub turn: Turn,
    pub idle: Idle,
    pub flee: Flee,
    pub tells: Tells,
}

impl Motion {
    /// Every package's `creature_motion.ron`, later ones replacing each part (and each
    /// style's and species' gaits) of the ones before.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut all = MotionFile::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let given: MotionFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            all.gaits.extend(given.gaits);
            all.species.extend(given.species);
            all.turn = given.turn.or(all.turn);
            all.idle = given.idle.or(all.idle);
            all.flee = given.flee.or(all.flee);
            all.tells = given.tells.or(all.tells);
        }
        let motion = Motion {
            gaits: all.gaits,
            species: all.species,
            turn: all.turn.ok_or(format!("no package's {FILE} says how creatures turn"))?,
            idle: all.idle.ok_or(format!("no package's {FILE} says how creatures stand about"))?,
            flee: all.flee.ok_or(format!("no package's {FILE} says how creatures flee"))?,
            tells: all.tells.ok_or(format!("no package's {FILE} gives the tells"))?,
        };
        for (_, name) in STYLES {
            motion.gaits.get(name).ok_or(format!("{FILE}: no gaits for the {name} style"))?;
        }
        if let Some(name) = motion.gaits.keys().find(|name| !STYLES.iter().any(|(_, s)| s == name)) {
            return Err(format!("{FILE}: `{name}` is not a movement style"));
        }
        motion.check()?;
        Ok(motion)
    }

    /// Species named in `species` that are not in the content.
    pub fn unknown_species<'a>(&'a self, known: impl Fn(&str) -> bool + 'a) -> impl Iterator<Item = &'a str> + 'a {
        self.species.keys().map(String::as_str).filter(move |name| !known(name))
    }

    fn check(&self) -> Result<(), String> {
        let mut problems = Vec::new();
        for (name, gaits) in self.gaits.iter().chain(&self.species) {
            for (which, gait) in [("walk", gaits.walk), ("run", gaits.run)] {
                if !(0.05..=1.4).contains(&gait.swing) {
                    problems.push(format!("{name}'s {which} swings {} radians (0.05 to 1.4)", gait.swing));
                }
                if !(0.2..=0.9).contains(&gait.duty) {
                    problems.push(format!("{name}'s {which} keeps a foot down {} of a stride (0.2 to 0.9)", gait.duty));
                }
                for (what, value, most) in [("bob", gait.bob, 0.4), ("pitch", gait.pitch.abs(), 0.5), ("lift", gait.lift, 0.6)] {
                    if !(0.0..=most).contains(&value) {
                        problems.push(format!("{name}'s {which} {what} is {value} (0 to {most})"));
                    }
                }
            }
        }
        let t = &self.turn;
        if !(0.5..=4.0).contains(&t.share) || !(0.02..=1.0).contains(&t.ease) || !(0.0..=1.0).contains(&t.head_lead) || !(0.0..=0.6).contains(&t.lean_most) || t.lean < 0.0 || t.step < 0.0 {
            problems.push(format!("the turn is out of bounds: {t:?}"));
        }
        let i = &self.idle;
        for (what, now) in [("glance", i.glance), ("sniff", i.sniff), ("flick", i.flick)] {
            if now.every < 0.5 || !(0.0..=1.0).contains(&now.chance) || now.hold <= 0.0 || now.hold > now.every || !(0.0..=1.5).contains(&now.size) {
                problems.push(format!("idle {what} is out of bounds: {now:?}"));
            }
        }
        if !(0.0..=0.2).contains(&i.shift) {
            problems.push(format!("idle shift {} (0 to 0.2)", i.shift));
        }
        let f = &self.flee;
        if !(0.0..=0.4).contains(&f.stretch) || !(1.0..=1.5).contains(&f.reach) || f.head.abs() > 1.0 || f.ears.abs() > 1.5 || f.tail.abs() > 1.5 {
            problems.push(format!("the flee is out of bounds: {f:?}"));
        }
        if !(0.0..=0.1).contains(&self.tells.heavy_tremble) || !(0.0..=2.0).contains(&self.tells.crawler_front) {
            problems.push(format!("the tells are out of bounds: {:?}", self.tells));
        }
        if problems.is_empty() { Ok(()) } else { Err(format!("{FILE}: {}", problems.join("; "))) }
    }

    /// A species' gaits (by its full name or its name alone): its own, else its style's.
    pub fn gaits(&self, species: &str, style: Style) -> Gaits {
        let own = self.species.get(species).or_else(|| species.rsplit(':').next().and_then(|name| self.species.get(name)));
        own.copied().unwrap_or_else(|| {
            let name = STYLES.iter().find(|(s, _)| *s == style).map_or("Walker", |(_, n)| n);
            self.gaits[name]
        })
    }
}

/// Where a body is between its walk and its run (0 ..= 1), from its speed and its
/// species' walking speed: walking up to a little over its walk, running by 1.6 times it.
pub fn run_share(speed: f32, walk: f32) -> f32 {
    let x = (speed / walk.max(0.2) - 1.15) / 0.45;
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The gait at a speed: the walk and the run mixed by [`run_share`], swinging less the
/// slower it goes under a walk (so a creature that ambles takes small steps, not slow
/// long ones), reaching further by `reach`. Also how far it is into a paired gait.
pub fn gait_at(gaits: &Gaits, speed: f32, walk: f32, reach: f32) -> (Gait, f32) {
    let run = run_share(speed, walk);
    let mut gait = gaits.walk.mix(gaits.run, run);
    let amble = (speed / walk.max(0.2)).clamp(0.0, 1.0).sqrt();
    gait.swing = (gait.swing * amble * reach).min(1.4);
    let paired = f32::from(u8::from(gaits.paired_walk)) * (1.0 - run) + f32::from(u8::from(gaits.paired_run)) * run;
    (gait, paired)
}

/// Blocks the body goes in one whole stride: each foot is down for `duty` of it and moves
/// back under the body `2 · travel` while it is (`travel` is `leg · sin swing`, or less for
/// a body whose shortest leg reaches less).
pub fn stride_length(travel: f32, gait: &Gait) -> f32 {
    2.0 * travel / gait.duty.max(0.05)
}

/// A leg `at` (0 ..= 1) of the way through its stride: its turn forward (radians, positive
/// with the foot ahead of the hip) and how much of it is drawn up. Down for the first
/// `duty` of the stride, the foot goes from ahead to behind in a straight line at the
/// ground's speed (so it does not slide); then it lifts and swings forward, easing in and
/// out. `reach` is half the foot's travel, in the leg's lengths (`sin swing` for the
/// stride's own leg, more for a shorter one).
pub fn leg(at: f32, gait: &Gait, reach: f32) -> (f32, f32) {
    let at = at.rem_euclid(1.0);
    let reach = reach.clamp(-0.99, 0.99);
    if at < gait.duty {
        let x = 1.0 - 2.0 * at / gait.duty;
        ((reach * x).asin(), 0.0)
    } else {
        let u = (at - gait.duty) / (1.0 - gait.duty).max(1e-3);
        let eased = u * u * (3.0 - 2.0 * u);
        ((reach * (2.0 * eased - 1.0)).asin(), gait.lift * (u * std::f32::consts::PI).sin())
    }
}

/// How high the body rides `at` its stride, 0 ..= 1 of its gait's bob: a walk is highest
/// as each foot passes under it, twice a stride; a hop or a bound once, in the air.
pub fn bob(at: f32, gait: &Gait, paired: f32) -> f32 {
    let at = at.rem_euclid(1.0);
    let tau = std::f32::consts::TAU;
    let walking = 0.5 + 0.5 * (2.0 * tau * (at - gait.duty * 0.5)).cos();
    let aloft = if at >= gait.duty { ((at - gait.duty) / (1.0 - gait.duty).max(1e-3) * std::f32::consts::PI).sin() } else { 0.0 };
    walking * (1.0 - paired) + aloft * paired
}

/// The phase a leg keeps in a paired gait: hind legs push off first, front ones half a
/// stride later; mixed from its own by how far into the paired gait the body is.
pub fn leg_phase(own: f32, hind: bool, paired: f32) -> f32 {
    let together = if hind { 0.0 } else { 0.5 };
    own + (together - own) * paired
}

/// Seconds-seeded now-and-then: whether `now` is inside one of these, and how far
/// through it (0 ..= 1). Each creature keeps its own time (`seed`).
pub fn now_and_then(at: &Now, now: f32, seed: u32, salt: u64) -> Option<f32> {
    let t = now / at.every + voxmar_engine::creature::unit_from(seed as u64 ^ salt) * 7.0;
    let slot = t.floor();
    let roll = voxmar_engine::creature::unit_from(voxmar_engine::creature::mix(seed as u64 ^ salt, slot as i64 as u64));
    let into = t.fract() * at.every;
    (roll < at.chance && into < at.hold).then_some(into / at.hold)
}

/// Its new yaw and turning speed, `dt` after: turning toward `toward` at up to `most`
/// radians a second, easing in over the last `ease` seconds' worth of the turn, its speed
/// itself changing smoothly.
pub fn turn_toward(yaw: f32, spin: f32, toward: f32, most: f32, ease: f32, dt: f32) -> (f32, f32) {
    let left = (toward - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let want = (left / ease.max(0.02)).clamp(-most, most);
    let spin = spin + (want - spin) * (14.0 * dt).min(1.0);
    // Never past where it was turning to.
    let step = spin * dt;
    let step = if left.abs() < step.abs() && left * step >= 0.0 { left } else { step };
    (yaw + step, spin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> Motion {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let files: Vec<ContentFile> = ["core", "base"]
            .into_iter()
            .filter_map(|package| std::fs::read_to_string(root.join(package).join(FILE)).ok().map(|source| ContentFile { package: package.into(), name: FILE.into(), source }))
            .collect();
        Motion::load(&files).expect("shipped creature motion loads")
    }

    #[test]
    fn the_shipped_motion_loads_with_every_style() {
        let motion = shipped();
        for (style, _) in STYLES {
            let gaits = motion.gaits("nobody", style);
            assert!(gaits.run.swing >= gaits.walk.swing, "{style:?} runs with longer swings than it walks");
        }
    }

    #[test]
    fn a_later_package_replaces_a_style_and_a_species_gets_its_own() {
        let core = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/core").join(FILE)).unwrap();
        let mine = "(gaits: {\"Hopper\": (walk: (swing: 0.2, duty: 0.5, bob: 0.0, lift: 0.0), run: (swing: 0.3, duty: 0.5, bob: 0.0, lift: 0.0))}, species: {\"rabbit\": (walk: (swing: 0.25, duty: 0.5, bob: 0.0, lift: 0.0), run: (swing: 0.35, duty: 0.5, bob: 0.0, lift: 0.0), paired_run: true)})";
        let files = vec![ContentFile { package: "core".into(), name: FILE.into(), source: core }, ContentFile { package: "mine".into(), name: FILE.into(), source: mine.into() }];
        let motion = Motion::load(&files).unwrap();
        assert_eq!(motion.gaits("hare", Style::Hopper).walk.swing, 0.2);
        assert_eq!(motion.gaits("rabbit", Style::Hopper).walk.swing, 0.25);
        assert_eq!(motion.unknown_species(|name| name != "rabbit").collect::<Vec<_>>(), ["rabbit"]);
        let bad = "(gaits: {\"Hopper\": (walk: (swing: 0.2, duty: 0.99, bob: 0.0, lift: 0.0), run: (swing: 0.3, duty: 0.5, bob: 0.0, lift: 0.0))})";
        let files = vec![files[0].clone(), ContentFile { package: "bad".into(), name: FILE.into(), source: bad.into() }];
        assert!(Motion::load(&files).unwrap_err().contains("foot down"));
    }

    /// The point of it all: through the part of the stride a foot is down, it stays where
    /// it was put on the ground, however fast the body goes, walking or running, for a long
    /// leg or a short one.
    #[test]
    fn a_foot_on_the_ground_does_not_slide() {
        let motion = shipped();
        for (style, _) in STYLES {
            let gaits = motion.gaits("nobody", style);
            for (walk, speed) in [(1.5, 0.6), (1.5, 1.5), (1.5, 2.4), (1.5, 6.0), (2.4, 9.0)] {
                for hip in [0.25, 0.9] {
                    let (gait, _) = gait_at(&gaits, speed, walk, 1.0);
                    let length = stride_length(hip * gait.swing.sin(), &gait);
                    // The body's way along and the foot's place on the ground, over a stance.
                    let foot = |at: f32| {
                        let body = at * length;
                        let (angle, _) = leg(at, &gait, gait.swing.sin());
                        body + hip * angle.sin()
                    };
                    let start = foot(0.0);
                    let mut worst: f32 = 0.0;
                    for i in 1..=50 {
                        let at = gait.duty * i as f32 / 50.0;
                        worst = worst.max((foot(at) - start).abs());
                    }
                    assert!(worst < 1e-3 * hip.max(length), "{style:?} at {speed} with a {hip} leg slid {worst} blocks over a stride of {length}");
                    // Forward through the swing: the foot comes back ahead, a stride on.
                    let landed = foot(1.0);
                    assert!((landed - start - length).abs() < 1e-3, "{style:?}: a stride on, the foot is {} ahead", landed - start);
                }
            }
        }
    }

    #[test]
    fn faster_means_quicker_and_longer_strides_and_a_still_body_stands_straight() {
        let motion = shipped();
        let gaits = motion.gaits("nobody", Style::Trotter);
        let cadence = |speed: f32| {
            let (gait, _) = gait_at(&gaits, speed, 2.0, 1.0);
            speed / stride_length(0.5 * gait.swing.sin(), &gait)
        };
        assert!(cadence(1.0) < cadence(2.0) && cadence(2.0) < cadence(7.0), "{} {} {}", cadence(1.0), cadence(2.0), cadence(7.0));
        let (still, _) = gait_at(&gaits, 0.0, 2.0, 1.0);
        assert_eq!(leg(0.3, &still, still.swing.sin()).0, 0.0);
        // Walking it moves its legs in turn, running together.
        assert_eq!(gait_at(&gaits, 2.0, 2.0, 1.0).1, 0.0);
        assert_eq!(gait_at(&gaits, 7.0, 2.0, 1.0).1, 1.0);
        assert_eq!(leg_phase(0.5, true, 1.0), 0.0);
        assert_eq!(leg_phase(0.5, true, 0.0), 0.5);
    }

    #[test]
    fn turning_is_smooth_bounded_and_never_overshoots() {
        let (mut yaw, mut spin) = (0.0f32, 0.0f32);
        let most = 3.0;
        let mut fastest: f32 = 0.0;
        let mut last_spin = 0.0f32;
        let mut jerk: f32 = 0.0;
        for _ in 0..240 {
            (yaw, spin) = turn_toward(yaw, spin, 2.5, most, 0.3, 1.0 / 60.0);
            fastest = fastest.max(spin.abs());
            jerk = jerk.max((spin - last_spin).abs());
            last_spin = spin;
            assert!(yaw <= 2.5 + 1e-4, "overshot to {yaw}");
        }
        assert!((yaw - 2.5).abs() < 1e-2, "reached {yaw}");
        assert!(fastest <= most + 1e-4, "turned at {fastest}");
        assert!(jerk < 1.0, "its turning speed jumped by {jerk} in a frame");
        // The short way round.
        let (yaw, _) = turn_toward(3.0, 0.0, -3.0, most, 0.3, 0.1);
        assert!(yaw > 3.0, "turned the long way: {yaw}");
    }

    #[test]
    fn now_and_then_comes_now_and_then() {
        let glance = Now { every: 4.0, chance: 0.4, hold: 1.2, size: 0.6 };
        let frames = 60 * 600;
        let on = (0..frames).filter(|f| now_and_then(&glance, *f as f32 / 60.0, 7, 1).is_some()).count() as f32 / frames as f32;
        let expected = glance.chance * glance.hold / glance.every;
        assert!((on - expected).abs() < 0.06, "on {on} of the time, {expected} expected");
        assert!(now_and_then(&glance, 3.0, 1, 1) != now_and_then(&glance, 3.0, 2, 1) || now_and_then(&glance, 9.0, 1, 1) != now_and_then(&glance, 9.0, 2, 1) || now_and_then(&glance, 17.0, 1, 1) != now_and_then(&glance, 17.0, 2, 1));
    }
}
