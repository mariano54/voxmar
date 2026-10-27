//! How a fight feels beyond its marks: what a blow does to what it lands on, to the view and
//! to the hand, and every fight sound (`combat_fx.ron`, in `core`; see [`style`]).
//!
//! The server decides every blow; this side only shows them, as the server's answers come
//! in ([`crate::fight::Fighting`]'s blows each way) and as the player swings, dodges and
//! guards:
//!
//! - A blow that lands freezes the creature a moment (a hit-stop, longer for a larger share
//!   of its health), flashes it white, then makes it flinch back and tip away and redden
//!   (drawn by `creatures`; it is never moved: there is no knockback). Sparks burst where it
//!   was struck and tufts of its coat fly, and the swing in hand holds still as long as the
//!   creature does, then the hand jolts back with the impact. A blow on the beat ticks,
//!   brighter and crisper; a flourish or a counter lands heavier and dips the view.
//! - A killing blow freezes it longer and dips the view; the body tips over, then shrinks
//!   into a puff of smoke.
//! - A creature coils through its wind-up and snaps into its strike as the blow lands
//!   (`creatures`, by its attack's style), so the timing to block or dodge reads in its body.
//! - A creature staggered (a perfect block, a daze) reels instead: its wind-up ends with no
//!   blow, its head droops and its body sways while the server holds it, dizzy sparks
//!   circle over its head and it thuds and wobbles (`combat_fx.ron`'s `stagger`).
//! - A blow that hurts the player thuds, tips the view toward the side it came from and
//!   reddens the edges of the screen. A block knocks and jolts the hand; a perfect block
//!   rings and throws sparks before the eyes.
//! - A boss's wind-up is told by ear: a sound swelling to its loudest as the blow lands
//!   (the music dips under it), and, for a blow whose reach the body cannot show, a ring on
//!   the ground as far as it reaches, filling in as the wind-up runs out (`tells`).
//! - Every fight sound is a voice of the mix's Combat category (`mix`).
//! - Sounds are synthesized at startup from the file's layers ([`sound`]), unless a package
//!   gives recordings.
//!
//! Screen Shake in the settings turns off every movement of the view.

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_engine::combat::{Blow, Defended, Style};
use voxmar_engine::creature::Role;

use crate::breaking::sky_light;
use crate::creatures::{Creatures, ShowCreatures};
use crate::drops::block_light;
use crate::fight::Fighting;
use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::render::particles::{Look, Motion, Particle, Particles};
use crate::session::Launch;
use crate::settings::Settings;
use crate::mix::{Category, Voice};
use crate::sfx::{Audible, MASTER};
use crate::world::ClientWorld;
use voxmar_engine::block::ParticleBlend;

pub mod shot;
pub mod sound;
pub mod style;

pub use style::FightLook;
use style::{Burst, Stance, StaggerLook, StrikeLook};

pub struct FightFxPlugin;

impl Plugin for FightFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Seen>()
            .init_resource::<HitStop>()
            .init_resource::<HandKick>()
            .init_resource::<CameraKick>()
            .init_resource::<Heard>()
            .init_resource::<Queued>()
            .add_systems(Startup, make_sounds)
            .add_systems(PostStartup, spawn_vignette)
            .init_resource::<Pending>()
            .init_resource::<shot::Shots>()
            .add_systems(Update, (react, tell).in_set(Reacted).after(ShowCreatures).run_if(in_state(Screen::InGame)))
            .add_systems(Update, shot::fly.after(Reacted).before(play).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), shot::reset)
            .add_systems(Update, markers.after(Reacted).run_if(in_state(Screen::InGame)))
            .add_systems(Update, play.after(Reacted).in_set(Audible))
            // What `react` set going shows the same frame.
            .add_systems(Update, (kick_camera.after(crate::player::follow_camera), vignette).after(Reacted).run_if(in_state(Screen::InGame)))
            .add_systems(OnEnter(Screen::InGame), warm_markers)
            .add_systems(OnExit(Screen::InGame), reset);
    }
}

/// Where blows are taken in (the freeze, the hand's knock, the view's kicks): what shows
/// them runs after, so they show the frame they land.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Reacted;

/// How long a blow's freeze lasts: the hit's share of its target's health makes it longer,
/// up to the most a hit freezes; the kind of blow scales it, and a killing blow freezes at
/// least as long as a death does.
pub fn stop_for(look: &FightLook, blow: Blow, share: f32, killed: bool) -> f32 {
    let hit = &look.hit;
    let stop = (hit.stop + hit.stop_per_share * share.clamp(0.0, 1.0)).min(hit.stop_most) * look.blow(blow).stop;
    if killed { stop.max(look.death.stop) } else { stop }
}

/// How big a creature's flinch is, as a share of the hit's `recoil`, `tilt` and `squash`:
/// one of `size` (width, height, already at its own scale) struck for `share` of its
/// health. A rabbit is thrown about, a bear barely rocked; a telling blow rocks harder than
/// a scratch.
pub fn flinch_size(weigh: &style::Weigh, size: (f32, f32), share: f32) -> f32 {
    let mass = (size.0 * size.0 * size.1).max(1e-3);
    let body = (weigh.mass / mass).cbrt().clamp(weigh.heavy, weigh.light);
    let blow = weigh.glancing + (weigh.telling - weigh.glancing) * (share / 0.5).clamp(0.0, 1.0);
    body * blow
}

/// How far a flinch `t` seconds in (of `seconds`) has the creature pushed back, 0 to 1:
/// quickly there, easing back.
pub fn flinch_weight(t: f32, seconds: f32) -> f32 {
    const THERE: f32 = 0.035;
    if t < 0.0 || t >= seconds {
        return 0.0;
    }
    if t < THERE {
        return t / THERE;
    }
    (1.0 - (t - THERE) / (seconds - THERE).max(1e-3)).max(0.0).powi(2)
}

/// A creature's strike stance: coiled `coil` of the way through its wind-up, or `striking`
/// seconds into its strike (snapping from the coil to the strike, then settling back, into
/// its recover stance if it has one, held until `recovery` seconds after the blow and eased
/// out by then, so the next wind-up starts from standing).
/// The second value is whether the strike is over (or there is none).
pub fn strike_stance(strike: &StrikeLook, coil: Option<f32>, striking: Option<f32>, recovery: f32) -> (Stance, bool) {
    let ease = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    if let Some(s) = striking {
        if s < strike.snap {
            let p = 1.0 - (1.0 - s / strike.snap).powi(3);
            return (strike.coil.scaled(1.0 - p).plus(strike.strike.scaled(p)), false);
        }
        let after = s - strike.snap;
        if strike.recover == Stance::default() {
            let settled = strike.strike.scaled(1.0 - ease(after / strike.settle));
            return if after < strike.settle { (settled, false) } else { (Stance::default(), true) };
        }
        // Out of the strike into the recover stance and out of that again, each over `settle`,
        // or over half of what the recovery leaves if that is shorter: the strike is gone by
        // the time the recover stance is whole, so a short recovery still reads.
        let room = (recovery - strike.snap).max(strike.settle);
        let ramp = strike.settle.min(room * 0.5);
        if after >= room {
            return (Stance::default(), true);
        }
        let settled = strike.strike.scaled(1.0 - ease(after / ramp));
        let held = ease(after / ramp).min(ease((room - after) / ramp));
        return (settled.plus(strike.recover.scaled(held)), false);
    }
    match coil {
        Some(c) => (strike.coil.scaled(ease(c)), true),
        None => (Stance::default(), true),
    }
}

/// How much of a creature's recover stance shows `striking` seconds into its strike (0 to
/// 1), for the gate to see that it stood its recovery out.
pub fn recover_share(strike: &StrikeLook, striking: f32, recovery: f32) -> f32 {
    if strike.recover == Stance::default() || striking < strike.snap {
        return 0.0;
    }
    let (stance, _) = strike_stance(&StrikeLook { strike: Stance::default(), ..*strike }, None, Some(striking), recovery);
    (stance.size() / strike.recover.size().max(1e-4)).min(1.0)
}

/// Seconds into a creature's strike that its recover stance is deepest: halfway through
/// what its recovery leaves after the snap.
pub fn recover_peak(strike: &StrikeLook, recovery: f32) -> f32 {
    strike.snap + (recovery - strike.snap).max(strike.settle) * 0.5
}

/// How deep a creature's coil shows `coil` of the way through its wind-up (0 to 1).
pub fn coil_share(strike: &StrikeLook, coil: f32) -> f32 {
    let (stance, _) = strike_stance(strike, Some(coil), None, 0.0);
    (stance.size() / strike.coil.size().max(1e-4)).min(1.0)
}

/// How far into its stagger stance a creature is (0 to 1, before easing) `dt` seconds of
/// game time on from `depth`: further in by `dt / ease` while `staggered`, as quickly back
/// out after. Counted in game time, so `ease` seconds after a stagger began it is all the
/// way in however the frames fell.
pub fn stagger_depth(look: &StaggerLook, depth: f32, staggered: bool, dt: f32) -> f32 {
    let by = dt / look.ease.max(1e-3);
    (if staggered { depth + by } else { depth - by }).clamp(0.0, 1.0)
}

/// How much of its stagger stance a creature `depth` into it (`stagger_depth`) shows, 0 to 1.
pub fn stagger_share(depth: f32) -> f32 {
    let x = depth.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A staggered creature's stance `depth` into it (`stagger_depth`), `t` seconds into the
/// stagger: the stagger's stance, its body swaying side to side.
pub fn stagger_stance(look: &StaggerLook, depth: f32, t: f32) -> Stance {
    let sway = look.sway * (std::f32::consts::TAU * look.sway_rate * t).sin();
    Stance { twist: look.stance.twist + sway, ..look.stance }.scaled(stagger_share(depth))
}

/// The sounds of a blow of the player's the server answered, by name and loudness: a hit by
/// the weapon's style (with the shell's clack on a shelled creature) and the kind's accent
/// over it, a miss's air, bleeding's drip.
pub fn struck_sounds(damage: u16, blow: Blow, killed: bool, style: Style, shell: bool) -> Vec<(String, f32)> {
    if blow == Blow::Bleed {
        return vec![("bleed".into(), 1.0)];
    }
    if damage == 0 {
        return vec![("miss".into(), 1.0)];
    }
    let mut sounds = vec![(format!("hit.{}", style.key()), 1.0)];
    if shell {
        sounds.push(("hit.shell".into(), 1.0));
    }
    match blow {
        Blow::Beat => sounds.push(("beat".into(), 1.0)),
        Blow::Flourish => sounds.extend([("beat".into(), 1.0), ("flourish".into(), 1.0)]),
        Blow::Counter => sounds.push(("counter".into(), 1.0)),
        Blow::Unaware => sounds.push(("unaware".into(), 1.0)),
        Blow::Plain | Blow::Bleed => {}
    }
    if killed {
        sounds.push(("kill".into(), 1.0));
    }
    sounds
}

/// The sounds of a creature's blow at the player, by what came of it.
pub fn hurt_sounds(damage: u16, defended: Defended) -> Vec<(&'static str, f32)> {
    match defended {
        Defended::Hit if damage > 0 => vec![("hurt", 1.0)],
        Defended::Hit | Defended::Missed | Defended::Dodged | Defended::Evaded => vec![("miss", 0.8)],
        Defended::Blocked if damage > 0 => vec![("blocked", 1.0), ("hurt", 0.45)],
        Defended::Blocked => vec![("blocked", 1.0)],
        Defended::Perfect => vec![("perfect", 1.0)],
    }
}

/// The sound a name plays: its own, else its family's (`swing.mace` falls back to `swing`).
pub fn resolve<'a>(look: &'a FightLook, name: &'a str) -> Option<&'a str> {
    if look.sounds.contains_key(name) {
        return Some(name);
    }
    let family = name.split('.').next()?;
    look.sounds.contains_key(family).then_some(family)
}

/// How long the swing in hand holds still with a blow that landed.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct HitStop {
    until: f32,
}

impl HitStop {
    pub fn holds(&self, now: f32) -> bool {
        now < self.until
    }
}

/// Knocks to the hand: where each started (app seconds), how far back and how long.
#[derive(Resource, Default, Debug, Clone)]
pub struct HandKick {
    kicks: Vec<(f32, f32, f32)>,
}

impl HandKick {
    /// Where the hand is knocked to now, in the hand's own blocks (right, up, forward): back
    /// toward the eye and a little down, there quickly and easing back.
    pub fn offset(&self, now: f32) -> Vec3 {
        // Knocked back at once, the frame the blow lands, and easing back.
        let back: f32 = self.kicks.iter().map(|&(at, by, seconds)| if (0.0..seconds).contains(&(now - at)) { by * (1.0 - (now - at) / seconds).powi(2) } else { 0.0 }).sum();
        Vec3::new(0.0, -back * 0.3, -back)
    }

    fn add(&mut self, now: f32, by: f32, seconds: f32) {
        self.kicks.retain(|&(at, _, s)| now - at < s);
        self.kicks.push((now, by, seconds));
    }
}

/// What moves the view: dips, tips and shakes, each with when it started.
#[derive(Resource, Default, Debug, Clone)]
pub struct CameraKick {
    kicks: Vec<Kick>,
    /// When the screen's edges last reddened.
    hurt_at: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
enum Kick {
    /// The view dips by degrees.
    Dip { at: f32, degrees: f32, seconds: f32 },
    /// The view tips (rolls) by degrees, positive to the right.
    Tip { at: f32, degrees: f32, seconds: f32 },
    /// The view shakes by degrees.
    Shake { at: f32, degrees: f32, seconds: f32 },
}

impl CameraKick {
    /// The view's turn now, in radians: yaw, pitch (down negative) and roll.
    pub fn turn(&self, now: f32) -> Vec3 {
        let mut turn = Vec3::ZERO;
        for kick in &self.kicks {
            match *kick {
                Kick::Dip { at, degrees, seconds } => turn.y -= degrees.to_radians() * flinch_weight(now - at, seconds),
                Kick::Tip { at, degrees, seconds } => turn.z -= degrees.to_radians() * flinch_weight(now - at, seconds),
                Kick::Shake { at, degrees, seconds } => {
                    let t = now - at;
                    if (0.0..seconds).contains(&t) {
                        let fade = (1.0 - t / seconds).powi(2) * degrees.to_radians();
                        turn.x += (t * 211.0).sin() * fade;
                        turn.y += (t * 173.0 + 1.3).sin() * fade;
                    }
                }
            }
        }
        turn
    }

    fn add(&mut self, kick: Kick) {
        let now = match kick {
            Kick::Dip { at, .. } | Kick::Tip { at, .. } | Kick::Shake { at, .. } => at,
        };
        self.kicks.retain(|k| match *k {
            Kick::Dip { at, seconds, .. } | Kick::Tip { at, seconds, .. } | Kick::Shake { at, seconds, .. } => now - at < seconds,
        });
        self.kicks.push(kick);
    }
}

/// Fight sounds asked for since joining, by name, played or not (a silent run plays none):
/// for the gate.
#[derive(Resource, Default, Debug)]
pub struct Heard {
    pub asked: std::collections::BTreeMap<String, u32>,
    /// Bursts of particles thrown: hits, perfect blocks and puffs; and of those, puffs.
    pub bursts: u32,
    pub poofs: u32,
    /// Times the view moved, and the hand was knocked.
    pub kicks: u32,
    pub knocks: u32,
    /// Rings drawn on the ground for bosses' blows.
    pub markers: u32,
    /// Shots loosed, specks of their trails left, and shots arrived; and shots of cancelled
    /// swings gone without arriving.
    pub shots: u32,
    pub trail: u32,
    pub impacts: u32,
    pub cancelled: u32,
}

/// Sounds to play this frame: name, loudness and pitch.
#[derive(Resource, Default)]
struct Queued(Vec<(String, f32, f32, bool)>);

/// Tells' sounds waiting for their moment (app seconds), so each one's loudest lands with
/// its blow, whose wind-up they tell and when that wind-up ends (its blow): one whose
/// wind-up ended well before its blow (the creature died, went, or stopped) is dropped.
#[derive(Resource, Default, Debug)]
pub struct Pending(pub Vec<Cue>);

/// A tell's sound waiting: when it plays (app seconds), what, whose wind-up it tells, how
/// loud (quieter further off), whether the music dips under it, and when the wind-up it
/// tells ends (its blow).
#[derive(Debug, Clone)]
pub struct Cue {
    pub due: f32,
    pub sound: String,
    pub id: u32,
    pub gain: f32,
    pub ducks: bool,
    pub blow: f32,
}

/// A ring on the ground telling how far a boss's blow reaches: whose, when its wind-up
/// began, how long it lasts, how far it reaches, and whether this is the filling disc
/// inside the ring rather than the ring.
#[derive(Component, Debug, Clone, Copy)]
pub struct Marker {
    pub id: u32,
    pub from: f32,
    pub windup: f32,
    pub reach: f32,
    pub fill: bool,
    alpha: f32,
}

/// Blocks within which a tell is heard at its full loudness.
const TOLD_NEAR: f32 = 6.0;

/// Seconds a tell's ring stays after the blow lands.
const MARKER_AFTER: f32 = 0.12;

/// How long before its end a wind-up seen to stop is its blow, not cut short: the pose is
/// seen late by up to the round trip, so the blow can end it this much early to the eye.
const BLOW_SLACK: f32 = 0.25;

/// What has been seen of the fight so far, to find what is new.
#[derive(Resource, Default)]
struct Seen {
    started: bool,
    struck: u32,
    hurt: u32,
    /// The rhythm's seconds into its swing last frame, and what was held.
    since: f32,
    held: Option<voxmar_engine::block::BlockId>,
    /// The player's cancelled swings (`Fighting::cancels`), and swings at the air
    /// (`Fighting::whiffs`).
    cancels: u32,
    whiffs: u32,
    dashing: bool,
    guarding: bool,
}

/// Every fight sound's takes, by name.
#[derive(Resource, Default)]
struct Bank {
    clips: FxHashMap<String, Vec<Handle<AudioSource>>>,
    gains: FxHashMap<String, f32>,
    counter: u64,
}

/// Makes every fight sound once, at startup, or loads its recordings.
fn make_sounds(mut commands: Commands, look: Option<Res<FightLook>>, assets: Res<AssetServer>, mut sources: ResMut<Assets<AudioSource>>) {
    let Some(look) = look else { return };
    let started = std::time::Instant::now();
    let mut bank = Bank::default();
    for (i, (name, def)) in look.sounds.iter().enumerate() {
        let clips = if def.recordings.is_empty() {
            sound::render(def, crate::sfx::mix(i as u64 * 7919 + 17))
                .iter()
                .map(|samples| sources.add(AudioSource { bytes: std::sync::Arc::from(crate::sfx::wav(samples)) }))
                .collect()
        } else {
            def.recordings.iter().map(|path| assets.load::<AudioSource>(path.clone())).collect()
        };
        bank.clips.insert(name.clone(), clips);
        bank.gains.insert(name.clone(), def.gain);
    }
    info!("fight sounds: {} made in {} ms", bank.clips.len(), started.elapsed().as_millis());
    commands.insert_resource(bank);
}

fn play(mut commands: Commands, mut queued: ResMut<Queued>, bank: Option<ResMut<Bank>>, settings: Res<Settings>) {
    let Some(mut bank) = bank else {
        queued.0.clear();
        return;
    };
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    for (name, gain, pitch, ducks) in queued.0.drain(..) {
        bank.counter += 1;
        let roll = crate::sfx::mix(bank.counter);
        let Some(clips) = bank.clips.get(&name).filter(|c| !c.is_empty()) else { continue };
        let clip = clips[(roll % clips.len() as u64) as usize].clone();
        let jitter = ((roll >> 32) & 0xffff) as f32 / 65535.0;
        let volume = Volume::Linear(bank.gains[&name] * gain * master);
        commands.spawn((
            AudioPlayer(clip),
            PlaybackSettings { speed: pitch * (0.96 + 0.08 * jitter), ..PlaybackSettings::DESPAWN.with_volume(volume) },
            Voice { category: Category::Combat, at: None, ducks },
        ));
    }
}

/// Tells each wind-up begun: its sound timed so its loudest lands with the blow (a boss's
/// with the music dipping under it), quieter further off, and a ring on the ground for a
/// blow that reaches further than the body shows.
#[allow(clippy::too_many_arguments)]
fn tell(
    mut commands: Commands,
    time: Res<Time>,
    look: Option<Res<FightLook>>,
    mut creatures: ResMut<Creatures>,
    mut pending: ResMut<Pending>,
    mut queued: ResMut<Queued>,
    mut heard: ResMut<Heard>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    player: Res<LocalPlayer>,
) {
    let now = time.elapsed_secs();
    let told = std::mem::take(&mut creatures.told);
    let Some(look) = look else { return };
    for told in told {
        let Some(tell) = look.tell_for(&told.key, told.style) else { continue };
        let Some((_, at, _)) = creatures.body(told.id) else { continue };
        // Heard as well as a blow near at hand, fainter further off.
        let gain = (TOLD_NEAR / at.distance(player.position).max(TOLD_NEAR)).sqrt();
        *heard.asked.entry(tell.sound.clone()).or_default() += 1;
        // Seen first mid wind-up, how much of it is left is not known: told at once.
        let due = if told.late { now } else { now + (told.windup - tell.peak).max(0.0) };
        pending.0.push(Cue { due, sound: tell.sound.clone(), id: told.id, gain, ducks: tell.ducks, blow: now + told.windup });
        let Some(marker) = tell.marker else { continue };
        heard.markers += 1;
        let colour = |alpha: f32| StandardMaterial { base_color: marker.colour.color().with_alpha(alpha), unlit: true, alpha_mode: AlphaMode::Blend, cull_mode: None, ..default() };
        let ring = meshes.add(annulus(told.reach, (told.reach - marker.width).max(0.0)));
        let disc = meshes.add(annulus(1.0, 0.0));
        for (mesh, fill) in [(ring, false), (disc, true)] {
            commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(materials.add(colour(0.0))),
                Transform::from_translation(at + Vec3::Y * 0.04).with_scale(if fill { Vec3::ZERO } else { Vec3::ONE }),
                Marker { id: told.id, from: now, windup: told.windup, reach: told.reach, fill, alpha: marker.alpha },
                bevy::light::NotShadowCaster,
                DespawnOnExit(Screen::InGame),
            ));
        }
    }
    // A wind-up that stops near its end was the blow: a tell still to come plays with it.
    // One that stops sooner, or whose creature falls or goes, was cut short: its tell is
    // dropped. Judged by the time left to the blow, not to the cue (which starts its sound's
    // peak before the blow).
    for cue in std::mem::take(&mut pending.0) {
        let struck = !creatures.winding(cue.id);
        if struck && !(creatures.standing(cue.id) && cue.blow - now <= BLOW_SLACK) {
            continue;
        }
        if struck || cue.due <= now {
            queued.0.push((cue.sound, cue.gain, 1.0, cue.ducks));
        } else {
            pending.0.push(cue);
        }
    }
}

/// A tell's ground ring and disc compile as the world loads (`render::warm`), not the first
/// time a boss winds up a blow that reaches past its body: drawn as `tell` draws them, see-
/// through, unlit and two-sided. So does a loosed shot's missile, drawn as `shot` draws
/// one (a box, unlit and solid), not the first time a stone or an arrow flies.
fn warm_markers(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let material = materials.add(StandardMaterial { base_color: Color::WHITE.with_alpha(0.0), unlit: true, alpha_mode: AlphaMode::Blend, cull_mode: None, ..default() });
    crate::render::warm::warm(&mut commands, meshes.add(annulus(1.0, 0.5)), material, false, DespawnOnExit(Screen::InGame));
    let (mesh, material) = shot::missile(&mut meshes, &mut materials, 1.0, 1.0, Color::WHITE);
    crate::render::warm::warm(&mut commands, mesh, material, false, DespawnOnExit(Screen::InGame));
}

/// A flat ring on the ground, `outer` blocks across from its middle to its edge and open
/// inside `inner` (0: a disc), facing up.
fn annulus(outer: f32, inner: f32) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    const SIDES: u32 = 48;
    let mut positions = Vec::new();
    for i in 0..SIDES {
        let a = i as f32 / SIDES as f32 * std::f32::consts::TAU;
        let (sin, cos) = a.sin_cos();
        positions.push([cos * outer, 0.0, sin * outer]);
        positions.push([cos * inner, 0.0, sin * inner]);
    }
    let indices: Vec<u32> = (0..SIDES).flat_map(|i| {
        let (o, n) = (i * 2, (i + 1) % SIDES * 2);
        [o, o + 1, n, n, o + 1, n + 1]
    }).collect();
    let count = positions.len();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; count])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; count])
        .with_inserted_indices(Indices::U32(indices))
}

/// Tells' rings follow their boss and fill in as its wind-up runs out, then go.
fn markers(
    mut commands: Commands,
    time: Res<Time>,
    creatures: Res<Creatures>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut rings: Query<(Entity, &Marker, &mut Transform, &MeshMaterial3d<StandardMaterial>)>,
) {
    let now = time.elapsed_secs();
    for (entity, marker, mut transform, material) in &mut rings {
        let t = now - marker.from;
        let body = creatures.body(marker.id);
        // Its blow fills it at once; a wind-up cut short (it fell, went, or stopped well
        // before its end) takes it away.
        let struck = !creatures.winding(marker.id);
        let ended = struck && (!creatures.standing(marker.id) || t < marker.windup - BLOW_SLACK);
        if t > marker.windup + MARKER_AFTER || body.is_none() || ended {
            materials.remove(&material.0);
            commands.entity(entity).despawn();
            continue;
        }
        let filled = if struck { 1.0 } else { marker_fill(t, marker.windup) };
        if let Some((_, at, _)) = body {
            transform.translation = at + Vec3::Y * 0.04;
        }
        let alpha = if marker.fill {
            transform.scale = Vec3::splat(filled * marker.reach);
            marker.alpha * 0.4 * filled
        } else {
            marker.alpha * (0.35 + 0.65 * filled)
        };
        if let Some(mut m) = materials.get_mut(&material.0) {
            m.base_color = m.base_color.with_alpha(alpha);
        }
    }
}

/// How far a tell's ring has filled in, 0 to 1, `t` seconds into a wind-up of `windup`:
/// full as the blow lands, and held there a moment after.
pub fn marker_fill(t: f32, windup: f32) -> f32 {
    (t / windup.max(0.05)).clamp(0.0, 1.0)
}

/// The edges of the screen, reddened while the player is hurt.
#[derive(Component)]
pub struct Vignette;

fn spawn_vignette(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    // Clear in the middle, rising to full at the corners: a soft ring, in white for the
    // colour to tint.
    const SIZE: u32 = 64;
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (u, v) = ((x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0, (y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0);
            let edge = ((u * u + v * v).sqrt() - 0.55).max(0.0) / 0.85;
            let alpha = (edge.min(1.0).powf(1.4) * 255.0) as u8;
            pixels.extend([255, 255, 255, alpha]);
        }
    }
    let image = Image::new(Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 }, TextureDimension::D2, pixels, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    commands.spawn((
        Vignette,
        ImageNode { image: images.add(image), color: Color::NONE, ..default() },
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        Visibility::Hidden,
        GlobalZIndex(-1),
    ));
}

fn vignette(time: Res<Time>, look: Option<Res<FightLook>>, kick: Res<CameraKick>, mut node: Query<(&mut ImageNode, &mut Visibility), With<Vignette>>) {
    let (Some(look), Ok((mut image, mut shown))) = (look, node.single_mut()) else { return };
    let camera = &look.camera;
    let t = kick.hurt_at.map_or(f32::MAX, |at| time.elapsed_secs() - at);
    let alpha = if t < camera.vignette_seconds { camera.vignette_alpha * (1.0 - t / camera.vignette_seconds).powi(2) } else { 0.0 };
    let want = if alpha > 0.002 { Visibility::Inherited } else { Visibility::Hidden };
    if *shown != want {
        *shown = want;
    }
    if alpha > 0.002 {
        image.color = camera.vignette.color().with_alpha(alpha);
    }
}

/// Turns the camera by what kicks it, after it follows the player's eye.
fn kick_camera(time: Res<Time>, settings: Res<Settings>, kick: Res<CameraKick>, mut camera: Query<&mut Transform, With<Camera3d>>) {
    if !settings.screen_shake {
        return;
    }
    let turn = kick.turn(time.elapsed_secs());
    if turn == Vec3::ZERO {
        return;
    }
    if let Ok(mut transform) = camera.single_mut() {
        transform.rotation *= Quat::from_euler(EulerRot::YXZ, turn.x, turn.y, turn.z);
    }
}

fn reset(mut seen: ResMut<Seen>, mut kick: ResMut<CameraKick>, mut hand: ResMut<HandKick>, mut stop: ResMut<HitStop>, mut pending: ResMut<Pending>) {
    pending.0.clear();
    *seen = Seen::default();
    *kick = CameraKick::default();
    *hand = HandKick::default();
    *stop = HitStop::default();
}

/// Turns what is new in the fight into sounds, sparks, freezes and kicks.
#[allow(clippy::too_many_arguments)]
fn react(
    time: Res<Time>,
    launch: Res<Launch>,
    look: Option<Res<FightLook>>,
    settings: Res<Settings>,
    fighting: Res<Fighting>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    mut creatures: ResMut<Creatures>,
    mut particles: ResMut<Particles>,
    (mut seen, mut stop, mut hand, mut kick, mut heard, mut queued): (ResMut<Seen>, ResMut<HitStop>, ResMut<HandKick>, ResMut<CameraKick>, ResMut<Heard>, ResMut<Queued>),
    mut shots: ResMut<shot::Shots>,
) {
    let Some(look) = look else { return };
    let now = time.elapsed_secs();
    let mut say = |name: &str, gain: f32, pitch: f32, heard: &mut Heard| {
        if let Some(name) = resolve(&look, name) {
            *heard.asked.entry(name.to_owned()).or_default() += 1;
            queued.0.push((name.to_owned(), gain, pitch, false));
        }
    };
    // The fight so far is not news on joining.
    if !seen.started {
        (seen.started, seen.struck, seen.hurt, seen.cancels, seen.whiffs) = (true, fighting.struck_count, fighting.hurt_count, fighting.cancels, fighting.whiffs);
    }
    let combat = &launch.content.combat;
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let weapon = combat.wielded(held, |skill| launch.content.skills.level(player.skills.xp(skill)));
    let class = combat.class_of(&weapon).map_or(voxmar_engine::combat::FIST_CLASS, |class| class.name.as_str());
    // A swing starts: the rhythm's meter starts over (not for a switch of what is held, nor
    // a swing cancelled, which sets the meter back too).
    let timing = combat.timing(&weapon);
    let since = fighting.rhythm.meter(time.elapsed_secs_f64(), &timing) * timing.cooldown;
    let shooting = look.shots.get(class);
    let cancelled = fighting.cancels != seen.cancels;
    seen.cancels = fighting.cancels;
    // A swing at the air whooshes, its class's whiff (`whiff`, or `whiff.<class>`).
    if fighting.whiffs != seen.whiffs {
        seen.whiffs = fighting.whiffs;
        say(&format!("whiff.{class}"), 1.0, 1.0, &mut heard);
    }
    if held == seen.held && since < seen.since - 1e-3 && !cancelled {
        say(&format!("swing.{class}"), 1.0, 1.0, &mut heard);
        // Its shot, if it draws one, is the one a cancel takes back.
        shots.last = None;
        // A shot is drawn, if there is one to loose: it lets go to reach the creature
        // looked at as the wind-up ends.
        let ammo = combat.ammo(&weapon);
        let loaded = ammo.is_empty() || (0..voxmar_engine::inventory::SLOTS).any(|slot| player.inventory.get(slot).is_some_and(|stack| ammo.contains(&stack.item)));
        if let Some(shooting) = shooting.filter(|_| loaded) {
            let reach = weapon.reach + combat.feel.reach_slack;
            let at = creatures.looked_at(&launch, player.eye(), player.look_direction(), reach);
            let flight = at.map_or(0.0, |(_, distance)| distance / shooting.speed);
            shots.draw(now + shot::let_go(timing.windup, flight), class, at.map(|(id, _)| id), reach);
        }
    }
    (seen.since, seen.held) = (since, held);
    let dashing = fighting.dash.is_some();
    if dashing && !seen.dashing {
        say("dodge", 1.0, 1.0, &mut heard);
        // The view leans the way the player goes.
        if let Some((dir, ..)) = fighting.dash
            && settings.screen_shake
        {
            let side = if dir.dot(player.rotation() * Vec3::X) < -0.1 { -1.0 } else { 1.0 };
            kick.add(Kick::Tip { at: now, degrees: look.camera.dodge_lean * side, seconds: look.camera.dodge_seconds });
            heard.kicks += 1;
        }
    }
    let guarding = fighting.guard.is_some();
    if guarding && !seen.guarding {
        say("guard", 1.0, 1.0, &mut heard);
    }
    (seen.dashing, seen.guarding) = (dashing, guarding);

    let amount = particles.amount;
    let light_at = |at: Vec3| {
        let cell = at.floor().as_ivec3();
        [sky_light(&world, cell), block_light(&world, cell)]
    };
    let eye = player.eye();
    let shake = settings.screen_shake;
    // The player's blows the server answered.
    let new = (fighting.struck_count.wrapping_sub(seen.struck) as usize).min(fighting.struck.len());
    seen.struck = fighting.struck_count;
    for &(id, damage, blow, _, killed) in &fighting.struck[fighting.struck.len() - new..] {
        let body = creatures.body(id);
        let def = body.map(|(species, ..)| &launch.content.creatures.get(species).def);
        let parts = body.map(|(species, ..)| &launch.content.creatures.get(species).parts);
        let shell = parts.is_some_and(|parts| parts.iter().any(|p| p.role == Role::Shell));
        let height = body.zip(def).map_or(1.0, |((_, _, scale), def)| def.body.size.1 * scale);
        // A big creature's hits sound deeper, a small one's higher.
        let pitch = (1.0 / height.max(0.2)).powf(0.15).clamp(0.82, 1.25);
        let share = def.map_or(0.2, |def| f32::from(damage) / f32::from(def.health.max(1)));
        let loud = 0.75 + 0.25 * (share * 3.0).min(1.0);
        for (name, gain) in struck_sounds(damage, blow, killed, weapon.style, shell) {
            say(&name, gain * loud, pitch, &mut heard);
        }
        // A shot that missed sails on past; one that hit strikes home. Told whatever is in
        // hand now: a missile loosed before the weapon was put away still waits for its
        // answer (one with no missile waiting at that creature is dropped). Bleeding is no
        // answer to a shot.
        if blow != Blow::Bleed {
            shots.verdicts.push((id, damage > 0));
        }
        if damage == 0 || blow == Blow::Bleed {
            continue;
        }
        let kind = look.blow(blow);
        stop.until = stop.until.max(now + stop_for(&look, blow, share, killed));
        hand.add(now, look.camera.recoil, 0.18);
        heard.knocks += 1;
        let punch = kind.punch + if killed { look.death.punch } else { 0.0 };
        if shake && punch > 0.0 {
            kick.add(Kick::Dip { at: now, degrees: punch, seconds: look.camera.punch_seconds });
            heard.kicks += 1;
        }
        // Sparks where it was struck, on the side toward the eye, and tufts of its coat.
        let Some(((_, feet, _), def)) = body.zip(def) else { continue };
        let middle = feet + Vec3::Y * height * 0.55;
        let toward = (eye - middle).normalize_or(Vec3::Y);
        let at = middle + toward * (def.body.size.0 * 0.5).min(height * 0.6);
        let coat = def.body.colours.first().copied().unwrap_or([180, 160, 140]);
        let seed = (id as u64) << 20 ^ (now * 1000.0) as u64;
        burst(&mut particles, &look.hit.sparks, kind.sparks * amount, at, toward, coat, light_at(at), true, seed);
        let chips = if shell { def.body.colours.get(1).copied().unwrap_or(coat) } else { coat };
        burst(&mut particles, &look.hit.tufts, kind.sparks.min(1.5) * amount, at, toward, chips, light_at(at), false, seed ^ 0x5bd1);
        heard.bursts += 1;
    }
    // Creatures' blows at the player.
    let new = (fighting.hurt_count.wrapping_sub(seen.hurt) as usize).min(fighting.hurt.len());
    seen.hurt = fighting.hurt_count;
    for &(id, damage, defended, _) in &fighting.hurt[fighting.hurt.len() - new..] {
        for (name, gain) in hurt_sounds(damage, defended) {
            say(name, gain, 1.0, &mut heard);
        }
        let camera = &look.camera;
        match defended {
            Defended::Hit if damage > 0 => {
                kick.hurt_at = Some(now);
                if shake {
                    // Tipped toward the side the blow came from.
                    let from = creatures.body(id).map_or(Vec3::ZERO, |(_, feet, _)| feet - eye);
                    let right = player.rotation() * Vec3::X;
                    let side = if from.dot(right) >= 0.0 { 1.0 } else { -1.0 };
                    kick.add(Kick::Tip { at: now, degrees: camera.hurt_roll * side, seconds: camera.hurt_seconds });
                    heard.kicks += 1;
                }
            }
            Defended::Blocked | Defended::Perfect => {
                hand.add(now, camera.block_jolt, 0.22);
                heard.knocks += 1;
                if shake {
                    kick.add(Kick::Shake { at: now, degrees: camera.block_shake * if defended == Defended::Perfect { 0.6 } else { 1.0 }, seconds: 0.2 });
                    heard.kicks += 1;
                }
                if defended == Defended::Perfect {
                    let look_dir = player.look_direction().normalize_or(Vec3::NEG_Z);
                    let at = eye + look_dir * 0.7 + Vec3::Y * -0.1;
                    burst(&mut particles, &camera.perfect_sparks, amount, at, look_dir, [255, 236, 170], light_at(at), true, now.to_bits() as u64);
                    heard.bursts += 1;
                }
                if defended == Defended::Blocked && damage > 0 {
                    kick.hurt_at = Some(now);
                }
            }
            _ => {}
        }
    }
    // Bodies vanishing: a puff of smoke.
    for (at, height, coat) in std::mem::take(&mut creatures.poofs) {
        let mut poof = look.death.poof;
        poof.size *= height.clamp(0.4, 2.5);
        let grey = poof.color.map_or(coat, |c| [c.0, c.1, c.2]);
        smoke(&mut particles, &poof, amount, at, height, grey, light_at(at), at.to_array().iter().map(|v| v.to_bits() as u64).fold(7, |a, b| a ^ b.rotate_left(17)));
        heard.bursts += 1;
        heard.poofs += 1;
    }
    // Creatures staggered: a thud and a dazed wobble (deeper for a bigger one, as its hits
    // are), and sparks circling up over its head.
    for (head, height, coat) in std::mem::take(&mut creatures.staggers) {
        let pitch = (1.0 / height.max(0.2)).powf(0.15).clamp(0.82, 1.25);
        // Heard as a blow near at hand is, fainter further off.
        let gain = (TOLD_NEAR / head.distance(player.position).max(TOLD_NEAR)).sqrt();
        say("stagger", gain, pitch, &mut heard);
        let sparks = look.stagger.sparks;
        let colour = sparks.color.map_or(coat, |c| [c.0, c.1, c.2]);
        dizzy(&mut particles, &sparks, amount, head, height, colour, light_at(head), head.to_array().iter().map(|v| v.to_bits() as u64).fold(11, |a, b| a ^ b.rotate_left(23)));
        heard.bursts += 1;
    }
}

/// Throws a burst of `count` × `scale` particles out from `at`, mostly along `toward`: sparks
/// glow and fade fast, chips are lit and fall to the ground.
#[allow(clippy::too_many_arguments)]
fn burst(particles: &mut Particles, burst: &Burst, scale: f32, at: Vec3, toward: Vec3, coat: [u8; 3], light: [u8; 2], glow: bool, seed: u64) {
    let count = (burst.count as f32 * scale).round() as u32;
    let mut noise = crate::sfx::Noise(seed);
    let colour = burst.color.map_or(coat, |c| [c.0, c.1, c.2]);
    for _ in 0..count {
        let random = Vec3::new(noise.next(), noise.next(), noise.next());
        let direction = (toward * 0.8 + random + Vec3::Y * 0.4).normalize_or(Vec3::Y);
        let speed = burst.speed * (0.55 + 0.45 * noise.unit());
        // Each a little lighter or darker, as tufts of a coat are.
        let shade = 0.85 + 0.3 * noise.unit();
        let colour = colour.map(|c| (c as f32 * if glow { 1.0 } else { shade }).min(255.0) as u8);
        particles.spawn(&Particle {
            position: at + random * 0.08,
            velocity: direction * speed,
            motion: Motion::Ballistic,
            gravity: if glow { 5.0 } else { 14.0 },
            drag: if glow { 5.0 } else { 1.5 },
            floor: at.y - 2.0,
            size: burst.size * (0.7 + 0.6 * noise.unit()),
            grow: if glow { 0.4 } else { 1.0 },
            life: burst.life * (0.7 + 0.6 * noise.unit()),
            look: Look::Speck,
            color: [colour[0], colour[1], colour[2], 255],
            glow,
            cools: false,
            light,
            blend: if glow { ParticleBlend::Add } else { ParticleBlend::Cutout },
            hides: if glow { 0.3 } else { 0.0 },
            ..Particle::default()
        });
    }
}

/// Dizzy sparks over a staggered creature's head (`at`): set round it in a ring as wide as
/// it is tall allows, each going round the ring and drifting up as it fades.
#[allow(clippy::too_many_arguments)]
fn dizzy(particles: &mut Particles, sparks: &Burst, scale: f32, at: Vec3, height: f32, colour: [u8; 3], light: [u8; 2], seed: u64) {
    let count = (sparks.count as f32 * scale).round() as u32;
    let mut noise = crate::sfx::Noise(seed);
    let radius = (height * 0.3).clamp(0.2, 0.8);
    for i in 0..count {
        let angle = std::f32::consts::TAU * (i as f32 + 0.5 * noise.unit()) / count.max(1) as f32;
        let out = Vec3::new(angle.cos(), 0.0, angle.sin());
        // Round the ring, the same way for every spark.
        let round = Vec3::new(-angle.sin(), 0.0, angle.cos());
        particles.spawn(&Particle {
            position: at + out * radius + Vec3::Y * noise.next() * 0.05,
            velocity: round * sparks.speed * (0.8 + 0.4 * noise.unit()) + Vec3::Y * 0.25,
            motion: Motion::Rise,
            gravity: -0.4,
            drag: 1.2,
            size: sparks.size * (0.8 + 0.4 * noise.unit()),
            grow: 0.6,
            wander: 0.1,
            life: sparks.life * (0.8 + 0.4 * noise.unit()),
            look: Look::Speck,
            color: [colour[0], colour[1], colour[2], 255],
            glow: true,
            light,
            blend: ParticleBlend::Add,
            hides: 0.3,
            ..Particle::default()
        });
    }
}

/// A puff of smoke where a body vanished, over its height.
#[allow(clippy::too_many_arguments)]
fn smoke(particles: &mut Particles, poof: &Burst, scale: f32, at: Vec3, height: f32, colour: [u8; 3], light: [u8; 2], seed: u64) {
    let count = (poof.count as f32 * scale).round() as u32;
    let mut noise = crate::sfx::Noise(seed);
    for _ in 0..count {
        let out = Vec3::new(noise.next(), noise.next() * 0.5, noise.next()).normalize_or(Vec3::X);
        particles.spawn(&Particle {
            position: at + out * 0.3 * height.min(2.0) + Vec3::Y * noise.next() * height * 0.3,
            velocity: out * poof.speed * (0.6 + 0.6 * noise.unit()) + Vec3::Y * 0.6,
            motion: Motion::Rise,
            gravity: -0.6,
            drag: 2.5,
            size: poof.size * (0.7 + 0.6 * noise.unit()),
            grow: 2.2,
            wander: 0.12,
            life: poof.life * (0.7 + 0.6 * noise.unit()),
            look: Look::Speck,
            color: [colour[0], colour[1], colour[2], 210],
            light,
            blend: ParticleBlend::Veil,
            ..Particle::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The least wind-up a creature's blow may have (seconds): time to see it coming and
    /// answer it.
    const READABLE: f32 = 0.3;

    fn look() -> FightLook {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/core/combat_fx.ron")).unwrap();
        FightLook::load(&[voxmar_engine::content::ContentFile { package: "core".into(), name: style::FILE.into(), source }]).unwrap()
    }

    /// Every fight sound, each of its takes as the game plays it (its gain, every volume at
    /// 100%), is heard within the Combat band.
    #[test]
    fn fight_sounds_are_heard_within_the_combat_band() {
        let look = look();
        let heard = look.sounds.iter().filter(|(_, def)| def.recordings.is_empty()).flat_map(|(name, def)| {
            sound::render(def, 7).into_iter().enumerate().map(move |(i, take)| (format!("{name} #{i}"), crate::sfx::loudness(&take, 1) * def.gain * MASTER))
        });
        crate::mix::tests::all([crate::mix::tests::within(Category::Combat, heard)]);
    }

    /// Every creature blow in the content has a tell (a boss's its own), each winds up long
    /// enough to be read and answered, and each tell's sound is at its loudest when it says
    /// (`peak`), no later than its blow lands, so played that long before the blow it
    /// lands with it.
    #[test]
    fn every_blow_is_told_and_its_sound_peaks_with_the_blow() {
        let look = look();
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        assert_eq!(look.untold(&content.creatures), Vec::<String>::new(), "blows with no tell");
        for (_, creature) in content.creatures.iter() {
            let name = creature.def.name.rsplit(':').next().unwrap();
            let blows = creature.def.attack.iter().map(|a| (name.to_owned(), a)).chain(creature.def.moves.iter().map(|m| (format!("{name}.{}", m.name), &m.attack)));
            for (key, attack) in blows {
                let windup = attack.windup.unwrap_or(content.combat.feel.creature_windup);
                assert!(windup >= READABLE, "{key} winds up {windup} s, under the {READABLE} s a blow needs to be read");
                let tell = look.tell_for(&key, attack.style).unwrap();
                assert!(tell.peak <= windup + 1e-3, "{key}'s tell {} is loudest {} s in, after its blow lands at {windup} s", tell.sound, tell.peak);
            }
        }
        let window = (0.05 * crate::sfx::RATE as f32) as usize; // the loudness window (`sfx::loudness`)
        for (name, tell) in &look.tells {
            for take in sound::render(&look.sounds[&tell.sound], 3) {
                let energy = |at: usize| take[at..(at + window).min(take.len())].iter().map(|s| s * s).sum::<f32>();
                let loudest = (0..take.len().saturating_sub(window)).step_by(window / 8).max_by(|a, b| energy(*a).total_cmp(&energy(*b))).unwrap();
                let at = (loudest + window / 2) as f32 / crate::sfx::RATE as f32;
                assert!((at - tell.peak).abs() < 0.12, "{name}: {} is loudest {at:.2} s in, not at {} s", tell.sound, tell.peak);
            }
        }
        // A ring fills as the wind-up runs out, full as the blow lands.
        assert_eq!(marker_fill(0.0, 1.1), 0.0);
        assert!((marker_fill(0.55, 1.1) - 0.5).abs() < 1e-5);
        assert_eq!(marker_fill(1.2, 1.1), 1.0);
    }

    /// A small creature is thrown about by a blow and a big one barely rocked; a blow that
    /// takes much of its health rocks it harder than a scratch, and the weights stay inside
    /// their bounds however small or huge the body.
    #[test]
    fn small_creatures_flinch_more_and_telling_blows_more_than_scratches() {
        let look = look();
        let weigh = &look.hit.weigh;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let size = |name: &str| {
            let (_, creature) = content.creatures.iter().find(|(_, c)| c.def.name.rsplit(':').next() == Some(name)).unwrap_or_else(|| panic!("no {name}"));
            creature.def.body.size
        };
        let at = |name: &str, share: f32| flinch_size(weigh, size(name), share);
        let (rabbit, wolf, bear) = (at("rabbit", 0.25), at("wolf", 0.25), at("brown_bear", 0.25));
        assert!(rabbit > wolf * 1.4, "a rabbit flinches {rabbit}, hardly more than a wolf's {wolf}");
        assert!(bear < wolf * 0.8, "a bear flinches {bear}, hardly less than a wolf's {wolf}");
        assert!((0.7..1.4).contains(&wolf), "a wolf, the middling body, flinches {wolf}, far from the hit's own size");
        assert!(at("wolf", 0.05) < at("wolf", 0.5), "a scratch rocks a wolf as hard as a telling blow");
        assert_eq!(at("wolf", 0.5), at("wolf", 1.0), "a blow past half its health rocks no harder");
        let most = weigh.light * weigh.telling;
        let least = weigh.heavy * weigh.glancing;
        assert_eq!(flinch_size(weigh, (0.01, 0.01), 1.0), most);
        assert_eq!(flinch_size(weigh, (40.0, 40.0), 0.0), least);
    }

    #[test]
    fn bigger_blows_freeze_longer_and_kills_longest() {
        let look = look();
        let small = stop_for(&look, Blow::Plain, 0.05, false);
        let big = stop_for(&look, Blow::Plain, 0.6, false);
        assert!(big > small, "a blow taking more of its health freezes longer ({small} then {big})");
        assert!(stop_for(&look, Blow::Flourish, 0.05, false) > small, "a flourish freezes longer than a plain blow");
        assert!(stop_for(&look, Blow::Plain, 0.05, true) >= look.death.stop, "a killing blow freezes as a death does");
        // Never past a few frames, and bleeding not at all.
        for blow in [Blow::Plain, Blow::Beat, Blow::Flourish, Blow::Counter, Blow::Bleed, Blow::Unaware] {
            assert!(stop_for(&look, blow, 1.0, true) <= 0.25);
        }
        assert_eq!(stop_for(&look, Blow::Bleed, 0.5, false), 0.0);
    }

    #[test]
    fn a_flinch_comes_quickly_and_eases_back() {
        assert_eq!(flinch_weight(0.0, 0.2), 0.0);
        assert!((flinch_weight(0.035, 0.2) - 1.0).abs() < 1e-4);
        assert!(flinch_weight(0.1, 0.2) < 1.0 && flinch_weight(0.1, 0.2) > flinch_weight(0.15, 0.2));
        assert_eq!(flinch_weight(0.2, 0.2), 0.0);
    }

    /// A stagger is all the way in `ease` seconds of game time after it began, however the
    /// frames fell (a few long ones or many short), shows its stance whole there, sways, and
    /// eases back out as long once it is over.
    #[test]
    fn a_stagger_eases_in_by_game_time_sways_and_eases_out() {
        let look = look().stagger;
        for frames in [1, 2, 3, 7, 60] {
            let dt = look.ease / frames as f32;
            let depth = (0..frames).fold(0.0, |depth, _| stagger_depth(&look, depth, true, dt));
            assert!((depth - 1.0).abs() < 1e-4, "{frames} frames over the ease came to {depth}");
        }
        assert!(stagger_depth(&look, 0.0, true, look.ease * 0.5) < 1.0, "half the ease is not all the way in");
        // All the way in, it stands in the stagger's stance (no sway at its start), and sways
        // `sway` either way a quarter of a sway's time later.
        let stance = stagger_stance(&look, 1.0, 0.0);
        assert!((stance.nod - look.stance.nod).abs() < 1e-5 && (stance.crouch - look.stance.crouch).abs() < 1e-5 && (stance.lean - look.stance.lean).abs() < 1e-5);
        let quarter = 0.25 / look.sway_rate.max(1e-3);
        assert!((stagger_stance(&look, 1.0, quarter).twist - look.stance.twist - look.sway).abs() < 1e-4);
        assert!(look.stance.nod > 0.0 && look.stance.lean < 0.0 && look.sway > 0.0, "its head droops, its weight goes back and it sways");
        // Out again over as long.
        assert_eq!(stagger_depth(&look, 1.0, false, look.ease), 0.0);
        assert_eq!(stagger_stance(&look, 0.0, quarter), Stance::default());
    }

    /// The core recovery after a creature's blow (`combat.ron`'s `creature_recovery`).
    const RECOVERY: f32 = 0.6;

    #[test]
    fn a_strike_coils_snaps_and_settles() {
        let look = look();
        for style in [Style::Stab, Style::Slash, Style::Crush, Style::Ranged, Style::Magic] {
            let strike = look.strike(style);
            // Coiling deepens through the wind-up.
            let (half, _) = strike_stance(&strike, Some(0.5), None, RECOVERY);
            let (full, _) = strike_stance(&strike, Some(1.0), None, RECOVERY);
            assert!(full.lean.abs() >= half.lean.abs() && (full.lean - strike.coil.lean).abs() < 1e-4, "{style:?}");
            // It snaps from the coil to the strike, leaning forward, and settles to nothing.
            let (start, over) = strike_stance(&strike, None, Some(0.0), RECOVERY);
            assert!(!over && (start.lean - strike.coil.lean).abs() < 1e-4, "{style:?} starts its strike from its coil");
            let (snapped, _) = strike_stance(&strike, None, Some(strike.snap), RECOVERY);
            assert!(snapped.lean > 0.0 && (snapped.lean - strike.strike.lean).abs() < 1e-4, "{style:?} strikes forward");
            // It stands in its recover stance through the recovery, and is back to standing
            // by its end, so the next wind-up starts from standing.
            let room = (RECOVERY - strike.snap).max(strike.settle);
            let (_, over) = strike_stance(&strike, None, Some(strike.snap + room + 0.01), RECOVERY);
            assert!(over, "{style:?}'s strike ends");
            let (end, _) = strike_stance(&strike, None, Some(strike.snap + room - 0.001), RECOVERY);
            assert!(end.lean.abs() + end.nod.abs() + end.crouch.abs() < 0.02, "{style:?} eases out of its recovery: {end:?}");
            assert!(strike.recover != Stance::default(), "{style:?} shows its recovery");
            let middle = recover_peak(&strike, RECOVERY);
            assert!((middle - (strike.snap + room * 0.5)).abs() < 1e-6);
            assert!(recover_share(&strike, middle, RECOVERY) > 0.95, "{style:?} stands in its recover stance halfway through");
            // A short recovery (a fox's) still shows its recover stance whole, the strike gone.
            let short = 0.4;
            let room = (short - strike.snap).max(strike.settle);
            let (peak, _) = strike_stance(&strike, None, Some(strike.snap + room * 0.5), short);
            assert!((peak.lean - strike.recover.lean).abs() < 1e-3 && (peak.nod - strike.recover.nod).abs() < 1e-3, "{style:?}: {peak:?}");
            // Its feet plant to coil and the lunge darts.
            assert!(strike.plant < 0.5 && strike.dart > 1.5, "{style:?}");
            // Every strike lunges forward from a coil drawn back.
            assert!(strike.coil.lean < 0.0 && strike.strike.lean > 0.0, "{style:?}");
        }
        // A bite snaps the head down at its target; horns drop the head before they drive.
        assert!(look.strike(Style::Stab).strike.nod > look.strike(Style::Stab).coil.nod);
        assert!(look.strike(Style::Crush).coil.nod > 0.0);
        // Claws rear up and swipe their front limbs down and forward.
        let claws = look.strike(Style::Slash);
        assert!(claws.coil.rear > 0.5 && claws.strike.paw > 0.0 && claws.coil.paw < 0.0);
    }

    #[test]
    fn every_blow_and_outcome_has_its_sounds() {
        let look = look();
        let blows = [Blow::Plain, Blow::Beat, Blow::Flourish, Blow::Counter, Blow::Bleed, Blow::Unaware];
        let styles = [Style::Stab, Style::Slash, Style::Crush, Style::Ranged, Style::Magic];
        for blow in blows {
            for style in styles {
                for (damage, killed, shell) in [(0, false, false), (3, false, false), (3, true, true)] {
                    for (name, gain) in struck_sounds(damage, blow, killed, style, shell) {
                        assert!(resolve(&look, &name).is_some(), "no sound for {name}");
                        assert!(gain > 0.0);
                    }
                }
            }
        }
        // A hit sounds its weapon's style; on the beat it ticks; a kill thumps; a miss is air.
        assert_eq!(struck_sounds(3, Blow::Beat, false, Style::Slash, false).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["hit.slash", "beat"]);
        assert!(struck_sounds(3, Blow::Plain, true, Style::Crush, true).iter().any(|(n, _)| n == "kill"));
        assert!(struck_sounds(3, Blow::Plain, false, Style::Crush, true).iter().any(|(n, _)| n == "hit.shell"));
        assert_eq!(struck_sounds(0, Blow::Plain, false, Style::Stab, false)[0].0, "miss");
        for defended in [Defended::Hit, Defended::Missed, Defended::Blocked, Defended::Perfect, Defended::Dodged, Defended::Evaded] {
            for damage in [0, 4] {
                for (name, _) in hurt_sounds(damage, defended) {
                    assert!(resolve(&look, name).is_some(), "no sound for {name}");
                }
            }
        }
        assert_eq!(hurt_sounds(4, Defended::Perfect)[0].0, "perfect");
        assert_eq!(hurt_sounds(4, Defended::Hit)[0].0, "hurt");
        // A class with no swing of its own whooshes as any weapon does.
        assert_eq!(resolve(&look, "swing.halberd"), Some("swing"));
        assert_eq!(resolve(&look, "swing.mace"), Some("swing.mace"));
    }

    #[test]
    fn kicks_ease_back_to_still() {
        let mut kick = CameraKick::default();
        kick.add(Kick::Dip { at: 0.0, degrees: 1.0, seconds: 0.2 });
        kick.add(Kick::Tip { at: 0.0, degrees: 4.0, seconds: 0.4 });
        kick.add(Kick::Shake { at: 0.0, degrees: 0.7, seconds: 0.2 });
        let peak = kick.turn(0.035);
        assert!(peak.y < 0.0 && peak.z < 0.0, "the view dips and tips: {peak}");
        assert_eq!(kick.turn(1.0), Vec3::ZERO, "and comes back to still");
        let mut hand = HandKick::default();
        hand.add(0.0, 0.05, 0.2);
        assert!(hand.offset(0.0).z < -0.049, "the hand is knocked back the frame the blow lands (the gate looks then)");
        assert!(hand.offset(0.1).z < 0.0 && hand.offset(0.1).z > hand.offset(0.0).z, "and eases back");
        assert_eq!(hand.offset(0.5), Vec3::ZERO);
    }
}
