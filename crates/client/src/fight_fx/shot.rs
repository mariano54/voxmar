//! Shots from slings and bows, as this side shows them (`combat_fx.ron`'s `shots`). The
//! server judges a shot the moment its wind-up ends, along a clear line from the eye; here
//! the missile is loosed from the hand as the draw lets go, timed to reach the creature as
//! the blow is judged, and flies there leaving a trail, kicking the hand back as it goes.
//! Where it arrives it throws a puff (of the creature's coat, else dust) and sounds; one the
//! server says missed sails on past the creature to land beyond it.

use bevy::prelude::*;
use rustc_hash::FxHashMap;

use super::{Heard, Queued, resolve};
use crate::breaking::sky_light;
use crate::drops::block_light;
use crate::fight_fx::FightLook;
use crate::menu::Screen;
use crate::render::particles::{Look, Motion, Particle, Particles};
use crate::world::ClientWorld;
use voxmar_engine::block::ParticleBlend;

/// Shots drawn, loosed and flying.
#[derive(Resource, Default)]
pub struct Shots {
    /// Drawn and not yet loosed: when they let go (app seconds), the class, the creature
    /// aimed at with the most blocks the shot reaches, and the swing it is.
    drawn: Vec<(f32, String, Option<u32>, f32, u64)>,
    /// To fly from this frame.
    pub loosed: Vec<Loose>,
    /// The server's answers to shots, in the order it gave them: the creature shot at, and
    /// whether it hit (to strike) or missed (to sail past). Each answers the oldest missile
    /// at that creature still waiting for one.
    pub(super) verdicts: Vec<(u32, bool)>,
    /// Missiles loosed so far, numbering each in turn.
    loosed_count: u64,
    /// Swings that drew a shot so far, numbering each in turn; the latest one's, while it
    /// may yet be cancelled; and the player's cancelled swings (`Fighting::cancels`) seen.
    swings: u64,
    pub last: Option<u64>,
    cancels: u32,
    /// Swings cancelled (the guard went up, or what was held changed, before the blow): the
    /// server judges no blow, so their shots strike nothing and make no sound arriving.
    cancelled: Vec<u64>,
    /// Seconds each frame moves a shot on whatever the frame took: the gate's, so a slow
    /// machine shows the same flight (unset: the frame's own).
    pub step: Option<f32>,
}

/// A shot let go: the class it is, where from and to, the creature it is at, and where it
/// lands if it misses.
#[derive(Debug, Clone)]
pub struct Loose {
    pub class: String,
    pub from: Vec3,
    pub to: Vec3,
    pub at: Option<u32>,
    pub beyond: Vec3,
    /// The swing it was loosed by (`Shots::swing`; 0, none).
    pub swing: u64,
}

/// A missile in flight.
#[derive(Component, Debug, Clone)]
pub struct Shot {
    pub class: String,
    from: Vec3,
    to: Vec3,
    beyond: Vec3,
    pub at: Option<u32>,
    /// Blocks it has flown, and of its whole flight.
    pub gone: f32,
    pub length: f32,
    /// Blocks flown when it last left a speck of its trail.
    trailed: f32,
    pub missed: bool,
    /// The server said it hit; and the seconds it has waited at its mark for that word.
    hit: bool,
    waited: f32,
    /// Its number among the missiles loosed, so answers go to them in turn.
    serial: u64,
    /// The swing it was loosed by, and whether that swing was cancelled: it then sails on
    /// past and goes without a puff or a sound.
    swing: u64,
    pub cancelled: bool,
}

/// The most seconds a shot that reached its creature waits there for the server's word
/// (which comes a tick or more after the blow is judged, more on a laggy link) before it
/// strikes anyway: a miss heard in time sails on past instead.
const VERDICT_WAIT: f32 = 0.3;

impl Shots {
    /// A new swing's number, its shot's to be cancelled by (`Shots::last`).
    pub fn swing(&mut self) -> u64 {
        self.swings += 1;
        self.last = Some(self.swings);
        self.swings
    }

    /// A shot drawn at `at` (or nothing) by `class`, letting go at `when`.
    pub fn draw(&mut self, when: f32, class: &str, at: Option<u32>, reach: f32) {
        let swing = self.swing();
        self.drawn.push((when, class.to_owned(), at, reach, swing));
    }

    /// The shots drawn that let go by `now`, the player having cancelled `cancels` swings
    /// ever: a swing cancelled since (even as it lets go) takes its shot back first.
    fn due(&mut self, now: f32, cancels: u32) -> Vec<(f32, String, Option<u32>, f32, u64)> {
        self.cancels(cancels);
        let (due, waiting): (Vec<_>, Vec<_>) = self.drawn.drain(..).partition(|(when, ..)| *when <= now);
        self.drawn = waiting;
        due
    }

    /// The player has cancelled `cancels` swings ever: on a new one, the latest swing's
    /// shot is cancelled. Not yet loosed, it never is; flying, it strikes nothing.
    pub fn cancels(&mut self, cancels: u32) {
        if cancels == self.cancels {
            return;
        }
        self.cancels = cancels;
        if let Some(swing) = self.last.take() {
            self.drawn.retain(|drawn| drawn.4 != swing);
            self.loosed.retain(|loose| loose.swing != swing);
            self.cancelled.push(swing);
        }
    }
}

/// When a shot drawn for `windup` seconds lets go, so it flies the `flight` seconds to its
/// mark as the wind-up ends and the blow is judged; never before half the draw.
pub fn let_go(windup: f32, flight: f32) -> f32 {
    (windup - flight).clamp(windup * 0.5, windup.max(0.0))
}

/// How high a flight `length` blocks long with `arc` (blocks at 10 blocks) is, `t` (0 to 1)
/// of the way along.
pub fn rise(arc: f32, length: f32, t: f32) -> f32 {
    arc * (length / 10.0) * 4.0 * t * (1.0 - t)
}

/// The spot `to` from the hand at `from` and the most it reaches: the creature's middle if
/// nothing solid stands between, else the first solid block along the way, else as far as
/// it reaches straight on.
fn mark(world: &ClientWorld, eye: Vec3, toward: Vec3, reach: f32) -> Vec3 {
    let direction = (toward - eye).normalize_or(Vec3::NEG_Z);
    let solid = |p: IVec3| world.block(p).is_some_and(|b| world.registry.info(b).solid);
    let distance = eye.distance(toward);
    match voxmar_engine::physics::raycast(eye, direction, reach.max(distance), solid) {
        Some(hit) if hit.distance < distance => eye + direction * hit.distance,
        _ => toward,
    }
}

/// Lets drawn shots go, flies every shot, and shows where each arrives.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn fly(
    mut commands: Commands,
    time: Res<Time>,
    look: Option<Res<FightLook>>,
    launch: Res<crate::session::Launch>,
    world: Res<ClientWorld>,
    player: Res<crate::player::LocalPlayer>,
    creatures: Res<crate::creatures::Creatures>,
    (mut shots, mut heard, mut queued, mut hand, mut particles): (ResMut<Shots>, ResMut<Heard>, ResMut<Queued>, ResMut<super::HandKick>, ResMut<Particles>),
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut made: Local<FxHashMap<String, (Handle<Mesh>, Handle<StandardMaterial>)>>,
    fighting: Res<crate::fight::Fighting>,
    mut flying: Query<(Entity, &mut Shot, &mut Transform)>,
) {
    let Some(look) = look else { return };
    let now = time.elapsed_secs();
    let eye = player.eye();
    // Drawn shots letting go: from the hand, at the creature's middle (or what stands in
    // the way), and on past it as far as it reaches for one that misses.
    for (_, class, at, reach, swing) in shots.due(now, fighting.cancels) {
        let aim = player.look_direction().normalize_or(Vec3::NEG_Z);
        let middle = at.and_then(|id| creatures.centre(&launch, id)).filter(|(_, dying)| !dying).map(|(middle, _)| middle);
        let to = mark(&world, eye, middle.unwrap_or(eye + aim * reach), reach);
        let on = (to - eye).normalize_or(aim);
        let beyond = mark(&world, eye, eye + on * reach, reach);
        let right = player.rotation() * Vec3::X;
        let from = eye + right * 0.26 - Vec3::Y * 0.16 + aim * 0.55;
        shots.loosed.push(Loose { class, from, to, at: middle.and(at), beyond, swing });
    }
    for loose in std::mem::take(&mut shots.loosed) {
        let Some(shot) = look.shots.get(&loose.class) else { continue };
        hand.add(now, shot.kick, 0.16);
        heard.knocks += 1;
        heard.shots += 1;
        let (mesh, material) = made
            .entry(loose.class.clone())
            .or_insert_with(|| missile(&mut meshes, &mut materials, shot.width, shot.length, shot.colour.color()))
            .clone();
        let length = loose.from.distance(loose.to);
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(loose.from).looking_to((loose.to - loose.from).normalize_or(Vec3::NEG_Z), Vec3::Y),
            Shot { class: loose.class, from: loose.from, to: loose.to, beyond: loose.beyond, at: loose.at, gone: 0.0, length, trailed: 0.0, missed: false, hit: false, waited: 0.0, serial: shots.loosed_count, swing: loose.swing, cancelled: false },
            bevy::light::NotShadowCaster,
            DespawnOnExit(Screen::InGame),
        ));
        shots.loosed_count += 1;
    }
    // Shots of cancelled swings that are flying: missed, as the server judged no blow, they
    // fly on past from where they are.
    let cancelled = std::mem::take(&mut shots.cancelled);
    for (_, mut flight, transform) in &mut flying {
        if cancelled.contains(&flight.swing) && !flight.hit && !flight.missed {
            flight.cancelled = true;
            pass(&mut flight, transform.translation);
        }
    }
    // The server's answers, each to the oldest missile at that creature still waiting for
    // one: two shots at one creature take their own answers, not each other's. One with no
    // missile waiting (a blow struck by hand) is dropped.
    let verdicts = std::mem::take(&mut shots.verdicts);
    if !verdicts.is_empty() {
        let mut waiting: Vec<_> = flying.iter_mut().filter(|(_, flight, _)| flight.at.is_some() && !flight.missed && !flight.hit).collect();
        waiting.sort_by_key(|(_, flight, _)| flight.serial);
        for (id, struck) in verdicts {
            let Some(index) = waiting.iter().position(|(_, flight, _)| flight.at == Some(id)) else { continue };
            let (_, mut flight, transform) = waiting.remove(index);
            if struck {
                flight.hit = true;
            } else {
                // Missed: it flies on past, from where it is now, to land beyond.
                pass(&mut flight, transform.translation);
            }
        }
    }
    let step = shots.step.unwrap_or(time.delta_secs());
    let amount = particles.amount;
    for (entity, mut flight, mut transform) in &mut flying {
        let Some(shot) = look.shots.get(&flight.class) else {
            commands.entity(entity).despawn();
            continue;
        };
        let along = |gone: f32, flight: &Shot| {
            let t = (gone / flight.length.max(1e-3)).min(1.0);
            flight.from.lerp(flight.to, t) + Vec3::Y * rise(shot.arc, flight.length, t)
        };
        flight.gone = (flight.gone + shot.speed * step).min(flight.length);
        let here = along(flight.gone, &flight);
        let ahead = along((flight.gone + 0.05).min(flight.length), &flight) - along((flight.gone - 0.05).max(0.0), &flight);
        *transform = Transform::from_translation(here).looking_to(ahead.normalize_or(Vec3::NEG_Z), Vec3::Y);
        // Its trail: a speck every so far along the way it came this frame.
        let trail = shot.trail;
        let mut next = flight.trailed + trail.every;
        while next <= flight.gone {
            let at = along(next, &flight);
            let cell = at.floor().as_ivec3();
            particles.spawn(&Particle {
                position: at,
                velocity: Vec3::ZERO,
                motion: Motion::Ballistic,
                drag: 4.0,
                size: trail.size,
                grow: 0.35,
                life: trail.life,
                look: Look::Speck,
                color: [trail.colour.0, trail.colour.1, trail.colour.2, (trail.alpha * 255.0) as u8],
                light: [sky_light(&world, cell), block_light(&world, cell)],
                blend: ParticleBlend::Veil,
                ..Particle::default()
            });
            heard.trail += 1;
            flight.trailed = next;
            next += trail.every;
        }
        if flight.gone < flight.length {
            continue;
        }
        // At its creature before the server has said whether it hit: held there a moment
        // for the word, so a miss sails past rather than thudding home first.
        if flight.at.is_some() && !flight.missed && !flight.hit && flight.waited < VERDICT_WAIT {
            flight.waited += step;
            continue;
        }
        // A cancelled swing's: gone where it lands, without a word.
        if flight.cancelled {
            heard.cancelled += 1;
            commands.entity(entity).despawn();
            continue;
        }
        // Arrived: a puff and a sound, quieter the further off.
        let on_creature = flight.at.is_some() && !flight.missed;
        let coat = flight.at.filter(|_| on_creature).and_then(|id| creatures.body(id)).map(|(species, ..)| launch.content.creatures.get(species).def.body.colours.first().copied().unwrap_or([180, 160, 140]));
        let colour = coat.unwrap_or(shot.puff.color.map_or([170, 155, 130], |c| [c.0, c.1, c.2]));
        let toward = (eye - here).normalize_or(Vec3::Y);
        let cell = here.floor().as_ivec3();
        let mut puff = shot.puff;
        puff.color = None;
        super::burst(&mut particles, &puff, amount, here, toward, colour, [sky_light(&world, cell), block_light(&world, cell)], false, (here.to_array().iter().map(|v| v.to_bits() as u64).fold(11, |a, b| a ^ b.rotate_left(13))) ^ heard.shots as u64);
        heard.bursts += 1;
        heard.impacts += 1;
        let sound = if on_creature { &shot.hit } else { &shot.land };
        if let Some(name) = resolve(&look, sound) {
            *heard.asked.entry(name.to_owned()).or_default() += 1;
            let far = (6.0 / eye.distance(here).max(6.0)).sqrt();
            queued.0.push((name.to_owned(), far, 1.0, false));
        }
        commands.entity(entity).despawn();
    }
}

/// A shot that misses flies on past, from `here` where it is now, to land beyond.
fn pass(flight: &mut Shot, here: Vec3) {
    (flight.missed, flight.from, flight.to, flight.gone) = (true, here, flight.beyond, 0.0);
    flight.length = here.distance(flight.beyond);
    flight.trailed = 0.0;
}

/// Drops every shot drawn or flying on leaving the game.
pub(super) fn reset(mut shots: ResMut<Shots>) {
    *shots = Shots::default();
}

/// A shot's missile: a box `width` across and `length` long, unlit in `colour` (warmed as
/// the world loads in `fight_fx::warm_markers`).
pub(super) fn missile(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, width: f32, length: f32, colour: Color) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    (meshes.add(Cuboid::new(width, width, length)), materials.add(StandardMaterial { base_color: colour, unlit: true, ..default() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shot_lets_go_to_land_as_the_draw_ends() {
        // Near: loosed late in the draw, arriving as it ends.
        assert!((let_go(0.2, 0.05) - 0.15).abs() < 1e-6);
        // Far: never before half the draw.
        assert_eq!(let_go(0.2, 0.3), 0.1);
        // It rises over the middle of its flight, more over a longer one, and lands level.
        assert_eq!(rise(0.3, 10.0, 0.0), 0.0);
        assert!((rise(0.3, 10.0, 0.5) - 0.3).abs() < 1e-6);
        assert!(rise(0.3, 20.0, 0.5) > rise(0.3, 10.0, 0.5));
        assert_eq!(rise(0.3, 10.0, 1.0), 0.0);
    }

    #[test]
    fn a_cancelled_swing_takes_its_shot_back() {
        let mut shots = Shots::default();
        shots.draw(1.0, "bow", Some(7), 12.0);
        shots.draw(2.0, "bow", Some(7), 12.0);
        // No new cancel: nothing goes.
        shots.cancels(0);
        assert_eq!(shots.drawn.len(), 2);
        // The latest swing cancelled: its shot is never loosed, the earlier one still is,
        // and a flight of it would strike nothing.
        shots.cancels(1);
        assert_eq!(shots.drawn.iter().map(|d| d.4).collect::<Vec<_>>(), vec![1]);
        assert_eq!(shots.cancelled, vec![2]);
        assert_eq!(shots.last, None);
        // A swing started since drew no shot (`last` cleared): a cancel of it takes none.
        shots.cancels(2);
        assert_eq!(shots.drawn.len(), 1);
        assert_eq!(shots.cancelled, vec![2]);
        // One loosed but not yet flying is taken back too.
        let swing = shots.swing();
        shots.loosed.push(Loose { class: "bow".into(), from: Vec3::ZERO, to: Vec3::X, at: Some(7), beyond: Vec3::X * 2.0, swing });
        shots.cancels(3);
        assert!(shots.loosed.is_empty());
        assert_eq!(shots.cancelled, vec![2, swing]);
    }

    #[test]
    fn a_swing_cancelled_as_its_shot_lets_go_looses_nothing() {
        let mut shots = Shots::default();
        shots.draw(1.0, "bow", Some(7), 12.0);
        // Due this frame, cancelled this frame: never loosed.
        assert!(shots.due(1.0, 1).is_empty());
        assert!(shots.drawn.is_empty());
        // Not cancelled, it lets go when due and not before.
        shots.draw(2.0, "bow", Some(7), 12.0);
        assert!(shots.due(1.5, 1).is_empty());
        assert_eq!(shots.due(2.0, 1).len(), 1);
    }
}
