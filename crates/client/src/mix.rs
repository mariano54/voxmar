//! The mix: how loud each kind of sound is made to be, which sounds the music makes room
//! for, how many of a kind may play at once, and how a sound placed in the world fades with
//! distance. The numbers are each package's `sounds.ron` `mix`.
//!
//! - Loudness bands. Every clip we ship is heard at a loudness inside its category's band
//!   (a test measures them all, as the game plays them). A clip far quieter than the rest
//!   of its kind is lost; one far louder startles.
//! - Ducking. While a jingle plays (a quest finished, a level reached), or a boss calls or
//!   strikes, the music dips under it and comes back after.
//! - Voices. Every sound started is a [`Voice`] of a category (but the music and the
//!   ambience's beds, which crossfade on their own); past the category's cap the farthest,
//!   then the oldest, fades out at once to make room.
//! - Distance. Sounds of things the player can walk up to (a block broken, a creature's
//!   call or step) fade along one curve, [`falloff`], to silence at their reach, and are
//!   panned to the nearer ear. The ambience's birds and thunder are far off by design and
//!   fade by their own `reach` (half as loud there), never quite to nothing.

use std::collections::BTreeMap;

use bevy::audio::{AudioSink, AudioSinkPlayback, Volume};
use bevy::prelude::*;
use serde::Deserialize;

use crate::session::Launch;
use crate::sfx::Recordings;

/// Positions are scaled by this for Bevy's panning, so that every sound within reach keeps
/// its full volume there and only its side changes; [`falloff`] sets loudness.
pub(crate) const PAN_SCALE: f32 = 1.0 / 96.0;
/// The most voices a category may play at once (`voices`), so that no package's mix, nor a
/// crowded town, costs frames: the town bench (`ambience::tests::big_town_sound_cost`)
/// plays at most 6 of the ambience's at once against its 8.
pub(crate) const VOICES_MOST: usize = 32;
/// Seconds a voice made room for takes to fade out, so it stops without a click.
const MAKE_ROOM: f32 = 0.06;

pub struct MixPlugin;

impl Plugin for MixPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Duck>()
            .add_systems(Startup, load)
            .add_systems(PostUpdate, (number, make_room, fade_out, duck).chain());
    }
}

/// What kind of sound something is, for its loudness band, its cap and ducking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Category {
    /// Tracks of `music.ron`.
    Music,
    /// The ambience's beds and one-shots (`ambience.ron`).
    Ambience,
    /// The interface's clicks and taps.
    Ui,
    /// What the player does and touches: steps, blocks placed and broken, eating,
    /// crafting, doors, a splash.
    Everyday,
    /// Blows, blocks and dodges in a fight.
    Combat,
    /// The jingles of events: a quest finished, its rewards, a level reached.
    Jingles,
    /// Creatures' voices.
    Creatures,
    /// Thunder.
    Weather,
}

impl Category {
    pub const ALL: [Category; 8] =
        [Category::Music, Category::Ambience, Category::Ui, Category::Everyday, Category::Combat, Category::Jingles, Category::Creatures, Category::Weather];
}

/// One package's `mix`; a later package's bands and caps replace those it names, and its
/// ducking replaces the whole.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MixDef {
    /// Loudness each category is heard at, low to high, in dB below full scale: the RMS of a
    /// clip's loudest 50 ms (all of it for music and ambience beds,
    /// which go on) times the volume the game plays it at
    /// with every volume setting at 100%.
    pub(crate) bands: BTreeMap<Category, (f32, f32)>,
    /// Sounds of a category playing at once, at most.
    pub(crate) voices: BTreeMap<Category, usize>,
    pub(crate) duck: Option<DuckDef>,
}

/// How the music makes room.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DuckDef {
    /// Categories the music dips under while one plays.
    pub(crate) under: Vec<Category>,
    /// Whether it dips under a boss's own voice too.
    #[serde(default)]
    pub(crate) bosses: bool,
    /// The music's volume while dipped, against its usual.
    pub(crate) to: f32,
    /// Seconds to dip, and to come back.
    pub(crate) fall: f32,
    pub(crate) rise: f32,
}

impl MixDef {
    pub(crate) fn merge(&mut self, later: MixDef) {
        self.bands.extend(later.bands);
        self.voices.extend(later.voices);
        if later.duck.is_some() {
            self.duck = later.duck;
        }
    }

    /// Whether every category has a band and a cap that make sense, and the ducking too.
    pub(crate) fn check(&self) -> Result<(), String> {
        for category in Category::ALL {
            let Some(&(low, high)) = self.bands.get(&category) else { return Err(format!("mix: no loudness band for {category:?}")) };
            if !(low < high && high <= 0.0) {
                return Err(format!("mix: {category:?}'s band ({low}, {high}) is not low to high, at most 0 dB"));
            }
            if category != Category::Music && self.voices.get(&category).is_none_or(|&n| n == 0) {
                return Err(format!("mix: {category:?} may play no voices"));
            }
            if let Some(&n) = self.voices.get(&category).filter(|&&n| n > VOICES_MOST) {
                return Err(format!("mix: {category:?} may play {n} voices at once, past {VOICES_MOST}"));
            }
        }
        if let Some(duck) = &self.duck {
            if !(0.0..1.0).contains(&duck.to) || duck.fall <= 0.0 || duck.rise <= 0.0 {
                return Err("mix: the music dips to a volume from 0 up to (not) 1, over seconds above 0".into());
            }
        }
        Ok(())
    }

    /// Sounds of `category` playing at once, at most.
    pub(crate) fn cap(&self, category: Category) -> usize {
        self.voices.get(&category).copied().unwrap_or(usize::MAX)
    }
}

/// The mix in play.
#[derive(Resource, Default)]
pub(crate) struct Mix(pub(crate) MixDef);

/// How loud the music is against its usual, while it makes room (1: not at all).
#[derive(Resource)]
pub(crate) struct Duck(pub(crate) f32);

impl Default for Duck {
    fn default() -> Self {
        Duck(1.0)
    }
}

/// A sound playing, by kind; `at` is where it sounds from in the world, if anywhere.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct Voice {
    pub(crate) category: Category,
    pub(crate) at: Option<Vec3>,
    /// The music dips under it (a boss's voice).
    pub(crate) ducks: bool,
}

impl Voice {
    pub(crate) fn of(category: Category) -> Self {
        Voice { category, at: None, ducks: false }
    }

    pub(crate) fn at(category: Category, at: Vec3) -> Self {
        Voice { category, at: Some(at), ducks: false }
    }
}

/// When a voice started, against the others.
#[derive(Component)]
struct Started(u64);

/// A voice fading out to make room, with the volume it had.
#[derive(Component)]
struct MakingRoom {
    left: f32,
    from: Option<f32>,
}

/// How loud a sound placed `distance` blocks off is, against up close, for a sound heard
/// within `reach`: falling ever more gently to nothing there.
pub(crate) fn falloff(distance: f32, reach: f32) -> f32 {
    let near = (1.0 - distance / reach.max(1e-3)).clamp(0.0, 1.0);
    near * near
}

fn load(mut commands: Commands, launch: Res<Launch>) {
    let mix = Recordings::load(&launch.content.files).map(|recorded| recorded.mix).unwrap_or_else(|e| {
        error!("no mix: {e}");
        MixDef::default()
    });
    commands.insert_resource(Mix(mix));
}

fn number(mut commands: Commands, voices: Query<Entity, Added<Voice>>, mut count: Local<u64>) {
    for entity in &voices {
        *count += 1;
        commands.entity(entity).insert(Started(*count));
    }
}

/// Past a category's cap, the farthest voices, then the oldest, fade out at once.
fn make_room(
    mut commands: Commands,
    mix: Option<Res<Mix>>,
    voices: Query<(Entity, &Voice, &Started), Without<MakingRoom>>,
    listener: Query<&GlobalTransform, With<Camera3d>>,
) {
    let Some(mix) = mix else { return };
    let ear = listener.single().map(|t| t.translation()).ok();
    let mut by: BTreeMap<Category, Vec<(f32, u64, Entity)>> = BTreeMap::new();
    for (entity, voice, started) in &voices {
        let distance = voice.at.zip(ear).map_or(0.0, |(at, ear)| at.distance(ear));
        by.entry(voice.category).or_default().push((distance, started.0, entity));
    }
    for (category, mut playing) in by {
        let cap = mix.0.cap(category);
        if playing.len() <= cap {
            continue;
        }
        // Farthest first, then oldest.
        playing.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for &(_, _, entity) in &playing[..playing.len() - cap] {
            commands.entity(entity).insert(MakingRoom { left: MAKE_ROOM, from: None });
        }
    }
}

fn fade_out(mut commands: Commands, time: Res<Time>, mut fading: Query<(Entity, &mut MakingRoom, Option<&mut AudioSink>)>) {
    for (entity, mut fade, sink) in &mut fading {
        // Not started yet (or never will, in a silent run): nothing to hear stop.
        let Some(mut sink) = sink else {
            commands.entity(entity).despawn();
            continue;
        };
        let from = *fade.from.get_or_insert(sink.volume().to_linear());
        fade.left -= time.delta_secs();
        if fade.left <= 0.0 {
            commands.entity(entity).despawn();
        } else {
            sink.set_volume(Volume::Linear(from * fade.left / MAKE_ROOM));
        }
    }
}

/// Dips the music while a sound it makes room for plays, and brings it back after.
fn duck(mix: Option<Res<Mix>>, time: Res<Time>, voices: Query<&Voice, Without<MakingRoom>>, mut level: ResMut<Duck>) {
    let Some(duck) = mix.as_ref().and_then(|mix| mix.0.duck.as_ref()) else { return };
    let under = voices.iter().any(|voice| (voice.ducks && duck.bosses) || duck.under.contains(&voice.category));
    level.0 = toward(level.0, under, duck, time.delta_secs());
}

/// The music's level a moment `dt` later, dipping while `under` and coming back after.
fn toward(level: f32, under: bool, duck: &DuckDef, dt: f32) -> f32 {
    if under {
        (level - (1.0 - duck.to) * dt / duck.fall).max(duck.to)
    } else {
        (level + (1.0 - duck.to) * dt / duck.rise).min(1.0)
    }
}

/// Loudness `linear` in dB below full scale (the tests' measure).
#[cfg(test)]
pub(crate) fn db(linear: f32) -> f32 {
    20.0 * linear.max(1e-6).log10()
}

#[cfg(test)]
pub(crate) mod tests;
