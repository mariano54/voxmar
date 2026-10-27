//! What a package says in `combat_fx.ron`, read and checked at launch: the look of a hit, a
//! kind of blow, a death, a creature's strike and its stagger, the camera and the hand, and
//! the fight's sounds. Each part of a later package replaces the one before; `blows`, `strikes` and
//! `sounds` replace entry by entry.

use std::collections::BTreeMap;

use serde::Deserialize;
use voxmar_engine::combat::{Blow, Style};
use voxmar_engine::content::ContentFile;

use crate::hud::combat::{Rgb, blow_name};

/// The file a package says how fights look and sound in.
pub const FILE: &str = "combat_fx.ron";

/// Sound names every package set must give between them; the others (`swing.<class>`,
/// `hit.<style>`, `hit.shell`) are optional.
pub const SOUNDS: [&str; 14] = ["swing", "hit", "beat", "flourish", "counter", "kill", "miss", "bleed", "hurt", "blocked", "perfect", "dodge", "guard", "stagger"];

/// A burst of particles: how many, how fast they fly out, how long they last, how wide,
/// and their colour (unset: the creature's own coat).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Burst {
    pub count: u32,
    pub speed: f32,
    pub life: f32,
    pub size: f32,
    #[serde(default)]
    pub color: Option<Rgb>,
}

/// A blow landing on a creature.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HitLook {
    pub stop: f32,
    pub stop_per_share: f32,
    pub stop_most: f32,
    pub shake: f32,
    pub white: f32,
    pub red: f32,
    pub flinch: f32,
    pub recoil: f32,
    pub tilt: f32,
    pub squash: f32,
    /// How big the flinch is, by the creature and the blow (`fight_fx::flinch_size`); left
    /// out, every flinch is the hit's own size.
    #[serde(default)]
    pub weigh: Weigh,
    pub sparks: Burst,
    pub tufts: Burst,
}

/// How a flinch is sized: a creature `mass` cubic blocks big (its width squared by its
/// height) flinches as the hit says; a lighter one more, by the cube root of how much
/// lighter, at most `light` times; a heavier one less, at least `heavy` times. And by the
/// share of its health the blow took: `glancing` times for a scratch, up to `telling` times
/// for half its health or more.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weigh {
    pub mass: f32,
    pub light: f32,
    pub heavy: f32,
    pub glancing: f32,
    pub telling: f32,
}

impl Default for Weigh {
    /// Unweighed: every creature, struck for any share, flinches the hit's own size.
    fn default() -> Self {
        Weigh { mass: 1.0, light: 1.0, heavy: 1.0, glancing: 1.0, telling: 1.0 }
    }
}

/// What a kind of blow multiplies a hit by, and the camera's dip with it.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlowLook {
    pub stop: f32,
    pub sparks: f32,
    pub punch: f32,
}

/// A killing blow and the body's going.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeathLook {
    pub stop: f32,
    pub punch: f32,
    pub shrink: f32,
    pub poof: Burst,
}

/// A pose a creature's body takes on top of its own: head nod (radians, down positive),
/// crouch (of its height), rear (0 to 1), lean (of its length, forward positive), front
/// limbs reaching forward (radians) and the body turned aside (radians).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stance {
    pub nod: f32,
    pub crouch: f32,
    pub rear: f32,
    pub lean: f32,
    pub paw: f32,
    /// The body turned aside (radians, to its left positive): a swipe winds round and
    /// sweeps across.
    #[serde(default)]
    pub twist: f32,
}

impl Stance {
    pub fn scaled(self, by: f32) -> Stance {
        Stance { nod: self.nod * by, crouch: self.crouch * by, rear: self.rear * by, lean: self.lean * by, paw: self.paw * by, twist: self.twist * by }
    }

    /// How far it is from standing, every part together.
    pub fn size(self) -> f32 {
        self.nod.abs() + self.crouch.abs() + self.rear.abs() + self.lean.abs() + self.paw.abs() + self.twist.abs()
    }

    pub fn plus(self, other: Stance) -> Stance {
        Stance { nod: self.nod + other.nod, crouch: self.crouch + other.crouch, rear: self.rear + other.rear, lean: self.lean + other.lean, paw: self.paw + other.paw, twist: self.twist + other.twist }
    }
}

/// How a creature that attacks in one style winds up and strikes.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrikeLook {
    pub coil: Stance,
    pub strike: Stance,
    pub snap: f32,
    pub settle: f32,
    /// The stance it stands in as it recovers after the blow (the server holds it still for
    /// its attack's `recovery`): the player's moment to hit back. None: it settles straight
    /// back.
    #[serde(default)]
    pub recover: Stance,
    /// How closely it follows where the server has it while it winds up (1: as it does
    /// walking, 0: rooted to the spot), so it plants its feet to coil.
    #[serde(default = "one")]
    pub plant: f32,
    /// How much faster than walking it closes on where the server has it as the blow
    /// snaps out, so the lunge darts rather than slides.
    #[serde(default = "one")]
    pub dart: f32,
}

/// A creature staggered (a perfect block, a daze): the stance it reels in (head drooped, a
/// little crouched, its weight back), its body swaying side to side `sway` radians either
/// way `sway_rate` times a second, eased into over `ease` seconds and out of over as many
/// once the stagger is over; and dizzy sparks circling over its head as it begins.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaggerLook {
    pub stance: Stance,
    pub sway: f32,
    pub sway_rate: f32,
    pub ease: f32,
    pub sparks: Burst,
}

fn one() -> f32 {
    1.0
}

/// How a creature's wind-up is told beyond its stance: a sound whose loudest moment
/// (`peak` seconds in) lands as the blow does, so the ear knows when to block or dodge;
/// and, for a blow whose reach the body cannot show, a ring on the ground as far as it
/// reaches, filling as the wind-up runs out.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TellLook {
    pub sound: String,
    pub peak: f32,
    #[serde(default)]
    pub marker: Option<MarkerLook>,
    /// Whether the music dips under it: a boss's, not every wolf's.
    #[serde(default = "dips")]
    pub ducks: bool,
}

fn dips() -> bool {
    true
}

/// A ring on the ground: its colour, how opaque at its fullest, and how wide (blocks).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerLook {
    pub colour: Rgb,
    pub alpha: f32,
    pub width: f32,
}

/// A shot from a sling or a bow, flying from the hand to what it was loosed at: how fast
/// (blocks a second), how high it arcs over the way (blocks at the middle of a flight 10
/// blocks long, more for longer), the missile drawn (`length` along its flight by `width`
/// across, in `colour`), the trail it leaves, how far the hand kicks back as it is loosed,
/// the puff where it lands (unset colour: the coat of the creature it struck, else dust), and
/// the sounds it lands with on a creature (`hit`) or anything else (`land`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotLook {
    pub speed: f32,
    pub arc: f32,
    pub length: f32,
    pub width: f32,
    pub colour: Rgb,
    pub trail: TrailLook,
    pub kick: f32,
    pub puff: Burst,
    pub hit: String,
    pub land: String,
}

/// A shot's trail: a speck left every `every` blocks of its flight, `size` wide, fading over
/// `life` seconds, in `colour` at `alpha` (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrailLook {
    pub every: f32,
    pub size: f32,
    pub life: f32,
    pub colour: Rgb,
    pub alpha: f32,
}

/// The camera and the hand.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraLook {
    pub hurt_roll: f32,
    pub hurt_seconds: f32,
    pub vignette: Rgb,
    pub vignette_alpha: f32,
    pub vignette_seconds: f32,
    pub block_shake: f32,
    pub block_jolt: f32,
    pub recoil: f32,
    pub punch_seconds: f32,
    pub perfect_sparks: Burst,
    /// Degrees the view leans into a dodge, and the seconds it takes.
    pub dodge_lean: f32,
    pub dodge_seconds: f32,
}

/// One layer of a synthesized sound (see `combat_fx.ron`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Layer {
    Click { gain: f32 },
    Tone { from: f32, to: f32, seconds: f32, gain: f32 },
    Noise { low: f32, high: f32, attack: f32, seconds: f32, gain: f32 },
    Sweep { from: f32, to: f32, seconds: f32, gain: f32 },
    Ring { freq: f32, partials: Vec<f32>, seconds: f32, gain: f32 },
    Crackle { low: f32, high: f32, grains: u32, seconds: f32, gain: f32 },
    /// Another layer, starting `at` seconds in: a tell's snap at the end of its wind-up.
    Later { at: f32, layer: Box<Layer> },
}

impl Layer {
    /// Seconds it lasts, its tail included.
    pub fn seconds(&self) -> f32 {
        match self {
            Layer::Click { .. } => 0.006,
            Layer::Tone { seconds, .. } | Layer::Noise { seconds, .. } | Layer::Sweep { seconds, .. } | Layer::Ring { seconds, .. } | Layer::Crackle { seconds, .. } => *seconds,
            Layer::Later { at, layer } => at + layer.seconds(),
        }
    }
}

/// A fight sound: synthesized from layers, or one of the package's recordings.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundDef {
    pub gain: f32,
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub drive: f32,
    #[serde(default)]
    pub room: f32,
    #[serde(default = "default_variants")]
    pub variants: u32,
    /// Paths within the package, made asset paths (`content/<package>/...`) as it loads.
    #[serde(default)]
    pub recordings: Vec<String>,
}

fn default_variants() -> u32 {
    4
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FxFile {
    hit: Option<HitLook>,
    blows: BTreeMap<String, BlowLook>,
    death: Option<DeathLook>,
    strikes: BTreeMap<String, StrikeLook>,
    stagger: Option<StaggerLook>,
    camera: Option<CameraLook>,
    sounds: BTreeMap<String, SoundDef>,
    tells: BTreeMap<String, TellLook>,
    shots: BTreeMap<String, ShotLook>,
}

/// How fights look and sound, from every package's `combat_fx.ron`.
#[derive(bevy::prelude::Resource, Clone, Debug, PartialEq)]
pub struct FightLook {
    pub hit: HitLook,
    pub blows: BTreeMap<String, BlowLook>,
    pub death: DeathLook,
    pub strikes: BTreeMap<String, StrikeLook>,
    pub stagger: StaggerLook,
    pub camera: CameraLook,
    pub sounds: BTreeMap<String, SoundDef>,
    /// Wind-ups told by sound and marker, by the creature's name and its move's
    /// (`rootmother.grasp`), or its name alone for its plain attack.
    pub tells: BTreeMap<String, TellLook>,
    /// Shots by the weapon class that looses them (`sling`, `bow`).
    pub shots: BTreeMap<String, ShotLook>,
}

const BLOWS: [Blow; 6] = [Blow::Plain, Blow::Beat, Blow::Flourish, Blow::Counter, Blow::Bleed, Blow::Unaware];
const STYLES: [Style; 5] = [Style::Stab, Style::Slash, Style::Crush, Style::Ranged, Style::Magic];

impl FightLook {
    /// Reads every package's file in order, a later one replacing what it gives, and
    /// refuses a look missing a part, a kind of blow, a style or a sound, or with a number
    /// that cannot be.
    /// Blows with no tell: every boss's plain attack and moves need their own, and every
    /// other creature's blow one of its own or its style's (`style.<style>`).
    pub fn untold(&self, creatures: &voxmar_engine::creature::CreatureRegistry) -> Vec<String> {
        let mut missing = Vec::new();
        for (_, creature) in creatures.iter() {
            let name = creature.def.name.rsplit(':').next().unwrap_or(&creature.def.name);
            let blows = creature.def.attack.iter().map(|a| (name.to_owned(), a.style)).chain(creature.def.moves.iter().map(|m| (format!("{name}.{}", m.name), m.attack.style)));
            for (key, style) in blows {
                let told = if creature.def.boss { self.tells.contains_key(&key) } else { self.tell_for(&key, style).is_some() };
                if !told {
                    missing.push(key);
                }
            }
        }
        missing
    }

    /// The tell of the blow `key` (`name.move`, or `name` for a plain attack) struck in
    /// `style`: its own, else its creature's, else its style's.
    pub fn tell_for(&self, key: &str, style: Style) -> Option<&TellLook> {
        let name = key.split('.').next().unwrap_or(key);
        self.tells.get(key).or_else(|| self.tells.get(name)).or_else(|| self.tells.get(&format!("style.{}", style.key())))
    }

    /// Weapon classes that shoot (take ammunition) with no shot to show.
    /// The creatures whose death, as this draws it, lies fallen less than
    /// `voxmar_engine::creature::LIES`: from the killing blow's freeze and the tip over
    /// (`death.fall`) to starting to go (the last `death.shrink` of its time to its loot).
    pub fn unlain(&self, creatures: &voxmar_engine::creature::CreatureRegistry) -> Vec<String> {
        creatures
            .iter()
            .filter(|(_, c)| lies(&c.def.death, self.death.stop, self.death.shrink) < voxmar_engine::creature::LIES - 1e-4)
            .map(|(_, c)| c.def.name.clone())
            .collect()
    }

    pub fn unshot<'a>(&self, classes: impl IntoIterator<Item = &'a voxmar_engine::combat::ClassDef>) -> Vec<String> {
        classes.into_iter().filter(|c| c.ammo.is_some() && !self.shots.contains_key(&c.name)).map(|c| c.name.clone()).collect()
    }

    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut all = FxFile::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let mut given: FxFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            for sound in given.sounds.values_mut() {
                for path in &mut sound.recordings {
                    *path = format!("content/{}/{path}", file.package);
                }
            }
            all.hit = given.hit.or(all.hit);
            all.death = given.death.or(all.death);
            all.stagger = given.stagger.or(all.stagger);
            all.camera = given.camera.or(all.camera);
            all.blows.extend(given.blows);
            all.strikes.extend(given.strikes);
            all.sounds.extend(given.sounds);
            all.tells.extend(given.tells);
            all.shots.extend(given.shots);
        }
        let look = FightLook {
            hit: all.hit.ok_or(format!("no package's {FILE} gives the hit"))?,
            blows: all.blows,
            death: all.death.ok_or(format!("no package's {FILE} gives the death"))?,
            strikes: all.strikes,
            stagger: all.stagger.ok_or(format!("no package's {FILE} gives the stagger"))?,
            camera: all.camera.ok_or(format!("no package's {FILE} gives the camera"))?,
            sounds: all.sounds,
            tells: all.tells,
            shots: all.shots,
        };
        for blow in BLOWS.map(blow_name) {
            look.blows.get(blow).ok_or(format!("{FILE}: nothing for a {blow} blow"))?;
        }
        for style in STYLES.map(Style::key) {
            look.strikes.get(style).ok_or(format!("{FILE}: no strike for the {style} style"))?;
        }
        for sound in SOUNDS {
            look.sounds.get(sound).ok_or(format!("{FILE}: no `{sound}` sound"))?;
        }
        look.check()?;
        Ok(look)
    }

    /// Times and sizes that may not be negative or infinite, shares that must be shares,
    /// and sounds that can be made.
    fn check(&self) -> Result<(), String> {
        let (hit, death, camera) = (&self.hit, &self.death, &self.camera);
        let mut numbers = vec![
            ("hit.stop", hit.stop),
            ("hit.stop_per_share", hit.stop_per_share),
            ("hit.stop_most", hit.stop_most),
            ("hit.shake", hit.shake),
            ("hit.white", hit.white),
            ("hit.red", hit.red),
            ("hit.flinch", hit.flinch),
            ("hit.recoil", hit.recoil),
            ("hit.tilt", hit.tilt),
            ("death.stop", death.stop),
            ("death.punch", death.punch),
            ("camera.hurt_roll", camera.hurt_roll),
            ("camera.hurt_seconds", camera.hurt_seconds),
            ("camera.vignette_seconds", camera.vignette_seconds),
            ("camera.block_shake", camera.block_shake),
            ("camera.block_jolt", camera.block_jolt),
            ("camera.recoil", camera.recoil),
            ("camera.punch_seconds", camera.punch_seconds),
            ("camera.dodge_lean", camera.dodge_lean),
            ("camera.dodge_seconds", camera.dodge_seconds),
        ];
        for (name, blow) in &self.blows {
            numbers.extend([("blows.*.stop", blow.stop), ("blows.*.sparks", blow.sparks), ("blows.*.punch", blow.punch)]);
            if !(blow.stop >= 0.0 && blow.sparks >= 0.0 && blow.punch >= 0.0 && blow.stop.is_finite() && blow.sparks.is_finite() && blow.punch.is_finite()) {
                return Err(format!("{FILE}: the {name} blow's numbers may not be less than 0 or infinite"));
            }
        }
        for (name, value) in numbers {
            if !(value >= 0.0 && value.is_finite()) {
                return Err(format!("{FILE}: {name} may not be less than 0 or infinite ({value})"));
            }
        }
        // A hit's freeze is at most a few frames; longer reads as a stall.
        let weigh = hit.weigh;
        let sane = |v: f32| v.is_finite() && v > 0.0;
        if !sane(weigh.mass) || !(0.1..=1.0).contains(&weigh.heavy) || !(1.0..=4.0).contains(&weigh.light) || !(0.1..=1.0).contains(&weigh.glancing) || !(1.0..=3.0).contains(&weigh.telling) {
            return Err(format!(
                "{FILE}: hit.weigh has a mass above 0, heavy 0.1 to 1, light 1 to 4, glancing 0.1 to 1 and telling 1 to 3 (mass {}, heavy {}, light {}, glancing {}, telling {})",
                weigh.mass, weigh.heavy, weigh.light, weigh.glancing, weigh.telling
            ));
        }
        if hit.stop_most > 0.25 || death.stop > 0.25 {
            return Err(format!("{FILE}: a freeze longer than 0.25 s reads as a stall (hit.stop_most {}, death.stop {})", hit.stop_most, death.stop));
        }
        for (name, share) in [("hit.squash", hit.squash), ("death.shrink", death.shrink), ("camera.vignette_alpha", camera.vignette_alpha)] {
            if !(0.0..=0.9).contains(&share) {
                return Err(format!("{FILE}: {name} is a share from 0 to 0.9, not {share}"));
            }
        }
        for (name, burst) in [("hit.sparks", hit.sparks), ("hit.tufts", hit.tufts), ("death.poof", death.poof), ("stagger.sparks", self.stagger.sparks), ("camera.perfect_sparks", camera.perfect_sparks)] {
            if burst.count > 64 || !(burst.life > 0.0 && burst.life <= 4.0) || !(burst.size > 0.0 && burst.size <= 2.0) || !(burst.speed >= 0.0 && burst.speed <= 30.0) {
                return Err(format!("{FILE}: {name} must be at most 64 particles, living over 0 to 4 s, 0 to 2 blocks wide, at 0 to 30 blocks a second"));
            }
        }
        for (name, strike) in &self.strikes {
            if !(strike.snap > 0.0 && strike.snap <= 1.0 && strike.settle > 0.0 && strike.settle <= 2.0) {
                return Err(format!("{FILE}: the {name} strike snaps over 0 to 1 s and settles over 0 to 2 s"));
            }
            if !(0.0..=1.0).contains(&strike.plant) || !(1.0..=8.0).contains(&strike.dart) {
                return Err(format!("{FILE}: the {name} strike plants from 0 to 1 and darts from 1 to 8 times its walk's pace (plant {}, dart {})", strike.plant, strike.dart));
            }
            for stance in [strike.coil, strike.strike, strike.recover] {
                if !stance_fits(stance) {
                    return Err(format!("{FILE}: the {name} strike's stances turn at most 2 radians, move at most 2 heights, and rear from 0 to 1"));
                }
            }
        }
        let stagger = &self.stagger;
        if !(0.0..=1.0).contains(&stagger.sway) || !(0.0..=10.0).contains(&stagger.sway_rate) || !(stagger.ease > 0.0 && stagger.ease <= 1.0) {
            return Err(format!(
                "{FILE}: the stagger sways at most 1 radian, 0 to 10 times a second, and eases in and out over 0 to 1 s (sway {}, sway_rate {}, ease {})",
                stagger.sway, stagger.sway_rate, stagger.ease
            ));
        }
        if !stance_fits(stagger.stance) {
            return Err(format!("{FILE}: the stagger's stance turns at most 2 radians, moves at most 2 heights, and rears from 0 to 1"));
        }
        for (name, tell) in &self.tells {
            let sound = self.sounds.get(&tell.sound).ok_or(format!("{FILE}: the {name} tell's sound `{}` is not given", tell.sound))?;
            let longest = sound.layers.iter().map(Layer::seconds).fold(0.0, f32::max);
            if !(0.0..=longest.max(0.0)).contains(&tell.peak) && sound.recordings.is_empty() {
                return Err(format!("{FILE}: the {name} tell's sound peaks {} s in, past its end", tell.peak));
            }
            if let Some(marker) = tell.marker
                && (!(0.05..=1.0).contains(&marker.alpha) || !(0.02..=1.0).contains(&marker.width))
            {
                return Err(format!("{FILE}: the {name} tell's ring is 0.05 to 1 opaque and 0.02 to 1 blocks wide"));
            }
        }
        for (name, shot) in &self.shots {
            for sound in [&shot.hit, &shot.land] {
                self.sounds.get(sound).ok_or(format!("{FILE}: the {name} shot's sound `{sound}` is not given"))?;
            }
            let trail = &shot.trail;
            let fine = (5.0..=120.0).contains(&shot.speed)
                && (0.0..=3.0).contains(&shot.arc)
                && (0.02..=2.0).contains(&shot.length)
                && (0.01..=0.5).contains(&shot.width)
                && (0.0..=0.3).contains(&shot.kick)
                && (0.05..=4.0).contains(&trail.every)
                && (0.005..=0.5).contains(&trail.size)
                && (0.02..=2.0).contains(&trail.life)
                && (0.0..=1.0).contains(&trail.alpha);
            if !fine {
                return Err(format!("{FILE}: the {name} shot flies 5 to 120 blocks a second, arcs 0 to 3 blocks, is 0.02 to 2 long and 0.01 to 0.5 wide, kicks 0 to 0.3, and leaves a speck every 0.05 to 4 blocks 0.005 to 0.5 wide for 0.02 to 2 s at 0 to 1 opaque"));
            }
            let puff = shot.puff;
            if puff.count > 64 || !(puff.life > 0.0 && puff.life <= 4.0) || !(puff.size > 0.0 && puff.size <= 2.0) || !(puff.speed >= 0.0 && puff.speed <= 30.0) {
                return Err(format!("{FILE}: the {name} shot's puff must be at most 64 particles, living over 0 to 4 s, 0 to 2 blocks wide, at 0 to 30 blocks a second"));
            }
        }
        for (name, sound) in &self.sounds {
            check_sound(name, sound)?;
        }
        Ok(())
    }

    /// The look of a kind of blow.
    pub fn blow(&self, blow: Blow) -> BlowLook {
        self.blows[blow_name(blow)]
    }

    /// How a creature attacking in `style` strikes.
    pub fn strike(&self, style: Style) -> StrikeLook {
        self.strikes[style.key()]
    }
}

/// Whether a stance stays within what a body can show: each part turning at most 2 radians
/// or moving at most 2 heights, and rearing from 0 to 1.
fn stance_fits(stance: Stance) -> bool {
    let parts = [stance.nod, stance.crouch, stance.rear, stance.lean, stance.paw, stance.twist];
    parts.iter().all(|v| v.is_finite() && v.abs() <= 2.0) && (0.0..=1.0).contains(&stance.rear)
}

/// A sound that can be made: some layers or recordings, sensible gains, times and pitches.
fn layer_ok(layer: &Layer) -> bool {
    let audible = |hz: f32| (20.0..=12_000.0).contains(&hz);
    match layer {
        Layer::Click { gain } => (0.0..=2.0).contains(gain),
        Layer::Tone { from, to, seconds, gain } | Layer::Sweep { from, to, seconds, gain } => audible(*from) && audible(*to) && *seconds > 0.0 && *seconds <= 3.0 && (0.0..=2.0).contains(gain),
        Layer::Noise { low, high, attack, seconds, gain } => audible(*low) && audible(*high) && low < high && *attack >= 0.0 && attack < seconds && *seconds <= 3.0 && (0.0..=2.0).contains(gain),
        Layer::Ring { freq, partials, seconds, gain } => audible(*freq) && !partials.is_empty() && partials.iter().all(|p| *p > 0.0 && audible(freq * p)) && *seconds > 0.0 && *seconds <= 3.0 && (0.0..=2.0).contains(gain),
        Layer::Crackle { low, high, grains, seconds, gain } => audible(*low) && audible(*high) && low < high && (1..=200).contains(grains) && *seconds > 0.0 && *seconds <= 3.0 && (0.0..=2.0).contains(gain),
        Layer::Later { at, layer } => (0.0..=2.0).contains(at) && layer_ok(layer),
    }
}

fn check_sound(name: &str, sound: &SoundDef) -> Result<(), String> {
    if sound.layers.is_empty() && sound.recordings.is_empty() {
        return Err(format!("{FILE}: the {name} sound has no layers and no recordings"));
    }
    if !(0.0..=2.0).contains(&sound.gain) || !(0.0..=8.0).contains(&sound.drive) || !(0.0..=0.8).contains(&sound.room) {
        return Err(format!("{FILE}: the {name} sound's gain is 0 to 2, its drive 0 to 8 and its room 0 to 0.8"));
    }
    if !(1..=8).contains(&sound.variants) {
        return Err(format!("{FILE}: the {name} sound has 1 to 8 variants, not {}", sound.variants));
    }
    for layer in &sound.layers {
        if !layer_ok(layer) {
            return Err(format!("{FILE}: a layer of the {name} sound cannot be made: {layer:?} (pitches 20 to 12000 Hz, times over 0 to 3 s, gains 0 to 2)"));
        }
    }
    Ok(())
}

/// Seconds a body lies fallen: from its tip over, after the killing blow's `stop`, to
/// starting to go over the last `shrink` of its time to its loot.
fn lies(death: &voxmar_engine::creature::Death, stop: f32, shrink: f32) -> f32 {
    death.drops_after() * (1.0 - shrink) - death.fall - stop
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> ContentFile {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/core/combat_fx.ron")).unwrap();
        ContentFile { package: "core".into(), name: FILE.into(), source }
    }

    #[test]
    fn the_shipped_look_loads() {
        let look = FightLook::load(&[core()]).unwrap();
        assert!(look.sounds.contains_key("hit.slash") && look.sounds.contains_key("swing.mace"));
        // Every weapon class the content gives has a swing of its own, the sword the plain one.
        let combat = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/base/combat.ron")).unwrap();
        let classes = combat.split("classes: [").nth(1).and_then(|rest| rest.split("weapons: [").next()).unwrap();
        for class in classes.lines().filter_map(|l| l.trim_start().strip_prefix("(name: \"")).map(|rest| rest.split('"').next().unwrap()) {
            // A sword's is the plain swing every other weapon falls back to.
            assert!(class == "sword" || look.sounds.contains_key(&format!("swing.{class}")), "no swing sound for the {class} class");
        }
    }

    /// A death lies fallen a beat past its killing blow's freeze, its tip over and its going;
    /// a look that takes half its death to go leaves it no time to lie, and is refused.
    #[test]
    fn a_death_must_lie_a_beat() {
        let look = FightLook::load(&[core()]).unwrap();
        let death = voxmar_engine::creature::Death::default();
        assert!(lies(&death, look.death.stop, look.death.shrink) >= voxmar_engine::creature::LIES, "the shipped look lies {}", lies(&death, look.death.stop, look.death.shrink));
        assert!(lies(&death, look.death.stop, 0.5) < voxmar_engine::creature::LIES, "a shrink of 0.5 was let lie");
    }

    #[test]
    fn a_later_package_replaces_entries() {
        let later = ContentFile {
            package: "mod".into(),
            name: FILE.into(),
            source: r#"(blows: { "plain": (stop: 0.5, sparks: 2.0, punch: 0.1) }, sounds: { "kill": (gain: 0.5, recordings: ["sounds/kill.ogg"]) })"#.into(),
        };
        let look = FightLook::load(&[core(), later]).unwrap();
        assert_eq!(look.blow(Blow::Plain).stop, 0.5);
        assert_eq!(look.blow(Blow::Beat), FightLook::load(&[core()]).unwrap().blow(Blow::Beat));
        assert_eq!(look.sounds["kill"].recordings, vec!["content/mod/sounds/kill.ogg".to_owned()]);
    }

    #[test]
    fn what_cannot_be_is_refused() {
        let with = |source: &str| FightLook::load(&[core(), ContentFile { package: "mod".into(), name: FILE.into(), source: source.into() }]);
        assert!(with(r#"(sounds: { "hit": (gain: 0.5) })"#).unwrap_err().contains("no layers"));
        assert!(with(r#"(sounds: { "hit": (gain: 0.5, layers: [Tone(from: 5, to: 50, seconds: 0.1, gain: 1)]) })"#).unwrap_err().contains("cannot be made"));
        assert!(with(r#"(strikes: { "stab": (coil: (nod: 0, crouch: 0, rear: 2, lean: 0, paw: 0), strike: (nod: 0, crouch: 0, rear: 0, lean: 0, paw: 0), snap: 0.1, settle: 0.2) })"#).unwrap_err().contains("rear from 0 to 1"));
        let hit = r#"(hit: (stop: 0.5, stop_per_share: 0, stop_most: 0.5, shake: 0, white: 0, red: 0, flinch: 0, recoil: 0, tilt: 0, squash: 0,
            sparks: (count: 1, speed: 1, life: 1, size: 0.1), tufts: (count: 1, speed: 1, life: 1, size: 0.1)))"#;
        assert!(with(hit).unwrap_err().contains("stall"));
        let weighed = |weigh: &str| {
            format!(
                r#"(hit: (stop: 0.05, stop_per_share: 0.1, stop_most: 0.1, shake: 0, white: 0, red: 0.2, flinch: 0.2, recoil: 0.1, tilt: 0.1, squash: 0.1, weigh: {weigh},
            sparks: (count: 1, speed: 1, life: 1, size: 0.1), tufts: (count: 1, speed: 1, life: 1, size: 0.1)))"#
            )
        };
        let unweighed = with(&weighed("()").replace(", weigh: ()", "")).unwrap();
        assert_eq!(super::super::flinch_size(&unweighed.hit.weigh, (0.2, 0.2), 0.9), 1.0, "a hit with no weigh flinches its own size");
        assert!(with(&weighed("(mass: 0.5, light: 1.8, heavy: 0.4, glancing: 0.7, telling: 1.5)")).is_ok());
        for weigh in ["(mass: 0, light: 1.8, heavy: 0.4, glancing: 0.7, telling: 1.5)", "(mass: 0.5, light: 0.5, heavy: 0.4, glancing: 0.7, telling: 1.5)", "(mass: 0.5, light: 1.8, heavy: 2, glancing: 0.7, telling: 1.5)", "(mass: 0.5, light: 1.8, heavy: 0.4, glancing: 0, telling: 1.5)", "(mass: 0.5, light: 1.8, heavy: 0.4, glancing: 0.7, telling: 9)"] {
            assert!(with(&weighed(weigh)).unwrap_err().contains("hit.weigh"), "{weigh} let through");
        }
        let stagger = |sway: f32, rate: f32, ease: f32, rear: f32| {
            format!(
                "(stagger: (stance: (nod: 0.5, crouch: 0.1, rear: {rear}, lean: -0.1, paw: 0), sway: {sway}, sway_rate: {rate}, ease: {ease}, sparks: (count: 6, speed: 0.8, life: 0.7, size: 0.06)))"
            )
        };
        assert!(with(&stagger(0.1, 2.0, 0.15, 0.0)).is_ok());
        for (sway, rate, ease) in [(1.5, 2.0, 0.15), (0.1, 12.0, 0.15), (0.1, 2.0, 0.0), (0.1, 2.0, 1.5), (-0.1, 2.0, 0.15)] {
            assert!(with(&stagger(sway, rate, ease, 0.0)).unwrap_err().contains("the stagger sways"), "{sway} {rate} {ease}");
        }
        assert!(with(&stagger(0.1, 2.0, 0.15, 2.0)).unwrap_err().contains("the stagger's stance"));
        assert!(with("(stagger: (stance: (nod: 0, crouch: 0, rear: 0, lean: 0, paw: 0), sway: 0, sway_rate: 0, ease: 0.1, sparks: (count: 1, speed: 1, life: 1, size: 0.1), dizzy: 1))").is_err(), "an unknown field");
        // Nothing at all.
        assert!(FightLook::load(&[]).is_err());
    }
}
