//! The item in the player's hand, seen from their own eyes: a tool or a weapon, a block as a
//! small cube, any other item as a thin sprite given thickness, low in the right of the
//! view. It bobs as the player breathes and walks, comes up from below when the player
//! switches to it, and swings when they dig, strike or use something, timed by the same
//! things that time those (the mining in progress, the fight's rhythm, the right button).
//! The empty hand shows the player's own fist and forearm (`bare`), which punches, works
//! at blocks, uses things and guards as anything held does.
//!
//! It is drawn by `render::items`, as a dropped item is, except that it is placed in the
//! view and always in front, so no wall comes between it and the eye. It takes the light
//! where the player stands, so it is dark in a dark cave.
//!
//! How it is held is data, `held.ron` in a texture pack (the pack's own entries replace the
//! packs it builds on, one by one, and the game says so): a pose for each kind of thing
//! held, found by its tags (`sword`, `pickaxe`, `edible`, ...) and one each for a block and
//! any other item, and the strokes and stances they move through. [`Book`] is what a pack
//! file says; [`Handling`] is the whole of it, checked.

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::combat::Defended;

use crate::breaking::sky_light;
use crate::drops::{block_light, look_of};
use crate::fight::Fighting;
use crate::interact::Target;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::render::items::{Held, ItemModel, ItemModels, Shape};
use crate::render::particles::TexturePieces;
use crate::session::Launch;
use crate::world::ClientWorld;

/// How the hand's pose is named in a pack: `held.ron`.
pub const FILE: &str = "held.ron";

pub struct HeldPlugin;

impl Plugin for HeldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Handled>()
            .init_resource::<Motion>()
            .init_resource::<HeldClock>()
            .add_systems(Update, show.after(crate::drops::show).after(crate::fight_fx::Reacted).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut motion: ResMut<Motion>| *motion = Motion::default());
    }
}

/// A pose: where an item sits in the view and how big it is.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pose {
    /// Where its middle is, in blocks: right, up and forward of the eye.
    pub at: [f32; 3],
    /// How it is turned from facing the eye, in degrees: pitch, yaw and roll (the roll
    /// first, in its own plane).
    pub turn: [f32; 3],
    /// Its width in blocks.
    pub size: f32,
}

/// A stroke: what the pose gains at the stroke's peak (and the same of the turn), over
/// `seconds` from start to rest, after a wind-up if it has one.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub at: [f32; 3],
    pub turn: [f32; 3],
    pub seconds: f32,
    #[serde(default)]
    pub wind: Option<Wind>,
}

/// The wind-up before a stroke: what the pose gains drawn back at its fullest, reached over
/// the first `share` of the stroke's time; the stroke proper fills the rest, as the wind-up
/// lets go.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Wind {
    pub at: [f32; 3],
    pub turn: [f32; 3],
    pub share: f32,
}

/// How far into its wind-up and its stroke proper a stroke `progress` (0 to 1) of the way
/// through is: the wind-up rises over its share and lets go quickly as the stroke goes
/// out; without one, the stroke is the whole of it.
pub fn wound_weights(progress: f32, wind: Option<Wind>, skew: f32) -> (f32, f32) {
    let share = wind.map_or(0.0, |w| w.share);
    if share <= 0.0 {
        return (0.0, stroke_weight(progress, skew));
    }
    let p = progress.clamp(0.0, 1.0);
    if p < share {
        return (smoothstep(p / share), 0.0);
    }
    let after = (p - share) / (1.0 - share);
    ((1.0 - after / 0.25).max(0.0).powi(2), stroke_weight(after, skew))
}

/// A stance, held while it lasts (the guard up, a bite being eaten): what the pose gains
/// at full, which it eases to and from.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stance {
    pub at: [f32; 3],
    pub turn: [f32; 3],
}

/// What the hand does besides rest, each as a pack gives it; a pose that names none uses
/// the book's.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Moves {
    /// The swing of a blow at a creature.
    pub attack: Option<Stroke>,
    /// The swing of working at a block, repeated while the button is held.
    pub dig: Option<Stroke>,
    /// The dip of a right click: placing, using, eating.
    pub place: Option<Stroke>,
    /// The guard held up.
    pub guard: Option<Stance>,
    /// Food brought up to be eaten.
    pub eat: Option<Stance>,
    /// The hand left behind as the player dodges, as for a dodge to the right (a dodge to
    /// the left mirrors it).
    pub dodge: Option<Stroke>,
    /// The hand knocked by a blow the player took unguarded.
    pub flinch: Option<Stroke>,
}

/// How one kind of thing is held. Matched by its `tag` against the held item's tags; the
/// book's `cube` and `sprite` are the poses of whatever no tag fits (their tag is only a
/// label).
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Kind {
    pub tag: String,
    pub pose: Pose,
    #[serde(default)]
    pub moves: Moves,
}

/// The empty hand: the fist and forearm, drawn as a row of cubes (the fist first, at
/// `pose`, then each piece of the arm `spacing` widths further back along it, toward the
/// elbow), each with one texture on every face.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bare {
    pub pose: Pose,
    #[serde(default)]
    pub moves: Moves,
    /// The pieces' textures in the packs (`block/...`), the fist first.
    pub pieces: Vec<String>,
    pub spacing: f32,
}

impl Bare {
    /// Its pose and moves as a kind of thing held, labelled `bare`.
    pub fn kind(&self) -> Kind {
        Kind { tag: "bare".to_owned(), pose: self.pose, moves: self.moves }
    }

    /// Where each piece's middle is (right, up and forward of the eye) for the fist at `at`
    /// turned by `turn` (degrees), `size` wide: the fist first.
    pub fn pieces_at(&self, at: Vec3, turn: Vec3, size: f32) -> Vec<Vec3> {
        let rotation = Quat::from_scaled_axis(turn_vector(turn));
        // The view looks down -z; the model's +z is back toward the eye.
        let fist = Vec3::new(at.x, at.y, -at.z);
        (0..self.pieces.len())
            .map(|i| {
                let p = fist + rotation * Vec3::new(0.0, 0.0, i as f32 * self.spacing * size);
                Vec3::new(p.x, p.y, -p.z)
            })
            .collect()
    }
}

/// The hand's own motion, in blocks and seconds.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Hand {
    /// How far the hand rises and falls with a breath, and breaths a second.
    pub breath: f32,
    pub breath_rate: f32,
    /// How far it sways across and up with a step at a full walk, and steps a block.
    pub sway: [f32; 2],
    pub steps_per_block: f32,
    /// The speed, in blocks a second, that sways it fully.
    pub sway_speed: f32,
    /// How far below its place a newly held item starts, and how long it takes to come up.
    pub lowered: f32,
    pub raise_seconds: f32,
    /// How long a stance takes to come up or go down.
    pub stance_seconds: f32,
    /// The skew of a stroke: below 1 it goes out fast and comes back slowly.
    pub skew: f32,
    /// How quickly the light on it follows the light where the player stands, a second, and
    /// how often that light is measured again where they stand.
    pub light_rate: f32,
    pub measure_seconds: f32,
}

/// What a pack's `held.ron` says: any part of it, which replaces the same part of the packs
/// below.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Book {
    pub hand: Option<Hand>,
    pub moves: Moves,
    /// Poses by tag, the first whose tag the item has taking it.
    pub kinds: Vec<Kind>,
    pub cube: Option<Kind>,
    pub sprite: Option<Kind>,
    /// The empty hand; none, and it draws nothing.
    pub bare: Option<Bare>,
}

impl Book {
    pub fn parse(source: &str) -> Result<Self, String> {
        ron::from_str(source).map_err(|e| format!("{FILE}: {e}"))
    }

    /// `later` over this book: what it gives replaces what this has, a kind by its tag (a
    /// new kind goes first, so it wins over the older ones), and `note` is told what it
    /// replaced.
    pub fn overlay(&mut self, later: Book, note: &mut impl FnMut(String)) {
        fn put<T>(slot: &mut Option<T>, new: Option<T>, what: &str, note: &mut impl FnMut(String)) {
            if let Some(new) = new {
                if slot.replace(new).is_some() {
                    note(what.to_owned());
                }
            }
        }
        put(&mut self.hand, later.hand, "the hand's motion", note);
        put(&mut self.moves.attack, later.moves.attack, "the attack stroke", note);
        put(&mut self.moves.dig, later.moves.dig, "the dig stroke", note);
        put(&mut self.moves.place, later.moves.place, "the place stroke", note);
        put(&mut self.moves.guard, later.moves.guard, "the guard stance", note);
        put(&mut self.moves.eat, later.moves.eat, "the eat stance", note);
        put(&mut self.moves.dodge, later.moves.dodge, "the dodge stroke", note);
        put(&mut self.moves.flinch, later.moves.flinch, "the flinch stroke", note);
        put(&mut self.cube, later.cube, "the pose of a block", note);
        put(&mut self.sprite, later.sprite, "the pose of any other item", note);
        put(&mut self.bare, later.bare, "the empty hand", note);
        for kind in later.kinds {
            match self.kinds.iter().position(|k| k.tag == kind.tag) {
                Some(at) => {
                    note(format!("the pose of {}", kind.tag));
                    self.kinds[at] = kind;
                }
                None => self.kinds.insert(0, kind),
            }
        }
    }

    /// The book as the game uses it, if it is whole and sensible.
    pub fn finish(self) -> Result<Handling, Vec<String>> {
        let mut problems = Vec::new();
        let mut need = |what: &str, ok: bool| {
            if !ok {
                problems.push(format!("{FILE} gives no {what}"));
            }
        };
        need("hand", self.hand.is_some());
        need("attack stroke", self.moves.attack.is_some());
        need("dig stroke", self.moves.dig.is_some());
        need("place stroke", self.moves.place.is_some());
        need("guard stance", self.moves.guard.is_some());
        need("eat stance", self.moves.eat.is_some());
        need("dodge stroke", self.moves.dodge.is_some());
        need("flinch stroke", self.moves.flinch.is_some());
        need("pose for a block (cube)", self.cube.is_some());
        need("pose for other items (sprite)", self.sprite.is_some());
        let (Some(hand), Some(attack), Some(dig), Some(place), Some(guard), Some(eat), Some(dodge), Some(flinch), Some(cube), Some(sprite)) =
            (self.hand, self.moves.attack, self.moves.dig, self.moves.place, self.moves.guard, self.moves.eat, self.moves.dodge, self.moves.flinch, self.cube, self.sprite)
        else {
            return Err(problems);
        };
        let handling = Handling { hand, moves: AllMoves { attack, dig, place, guard, eat, dodge, flinch }, kinds: self.kinds, cube, sprite, bare: self.bare, bare_layers: Vec::new() };
        let problems = handling.problems();
        if problems.is_empty() { Ok(handling) } else { Err(problems) }
    }
}

/// Every move, whole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AllMoves {
    pub attack: Stroke,
    pub dig: Stroke,
    pub place: Stroke,
    pub guard: Stance,
    pub eat: Stance,
    pub dodge: Stroke,
    pub flinch: Stroke,
}

/// How things are held, whole and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Handling {
    pub hand: Hand,
    pub moves: AllMoves,
    pub kinds: Vec<Kind>,
    pub cube: Kind,
    pub sprite: Kind,
    pub bare: Option<Bare>,
    /// The texture layers of the empty hand's pieces, once the textures are loaded (none
    /// before, and the empty hand is not drawn).
    pub bare_layers: Vec<u16>,
}

/// The pack's handling of things, or none if no pack gave a whole one (then nothing is
/// drawn in the hand, and the game said why at launch).
#[derive(Resource, Default)]
pub struct Handled(pub Option<Handling>);

impl Handling {
    /// The kind a thing with these tags is held as: the first whose tag it has, else the
    /// pose of a cube or of a sprite.
    pub fn kind_of(&self, has_tag: impl Fn(&str) -> bool, shape: Shape) -> &Kind {
        self.kinds.iter().find(|k| has_tag(&k.tag)).unwrap_or(match shape {
            Shape::Cube => &self.cube,
            Shape::Sprite => &self.sprite,
        })
    }

    /// `kind`'s moves, its own over the book's.
    pub fn moves_of(&self, kind: &Kind) -> AllMoves {
        AllMoves {
            attack: kind.moves.attack.unwrap_or(self.moves.attack),
            dig: kind.moves.dig.unwrap_or(self.moves.dig),
            place: kind.moves.place.unwrap_or(self.moves.place),
            guard: kind.moves.guard.unwrap_or(self.moves.guard),
            eat: kind.moves.eat.unwrap_or(self.moves.eat),
            dodge: kind.moves.dodge.unwrap_or(self.moves.dodge),
            flinch: kind.moves.flinch.unwrap_or(self.moves.flinch),
        }
    }

    /// Every way the numbers are unusable, in words.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        let h = &self.hand;
        let mut check = |what: &str, ok: bool| {
            if !ok {
                problems.push(format!("{FILE}: {what}"));
            }
        };
        check("the hand's numbers must be finite", finite(&[h.breath, h.breath_rate, h.sway[0], h.sway[1], h.steps_per_block, h.sway_speed, h.lowered, h.raise_seconds, h.stance_seconds, h.skew, h.light_rate, h.measure_seconds]));
        check("the hand's raise_seconds, stance_seconds and sway_speed must be above 0", h.raise_seconds > 0.0 && h.stance_seconds > 0.0 && h.sway_speed > 0.0);
        check("the hand's skew must be between 0.2 and 3", (0.2..=3.0).contains(&h.skew));
        check("the hand's light_rate and measure_seconds must be above 0", h.light_rate > 0.0 && h.measure_seconds > 0.0);
        for (name, stroke) in [("attack", self.moves.attack), ("dig", self.moves.dig), ("place", self.moves.place), ("dodge", self.moves.dodge), ("flinch", self.moves.flinch)] {
            check(&format!("the {name} stroke must take 0.05 to 3 seconds"), stroke.seconds.is_finite() && (0.05..=3.0).contains(&stroke.seconds));
            check(&format!("the {name} stroke's wind-up must take 0 to 0.6 of it, in finite numbers"), stroke.wind.is_none_or(|w| (0.0..=0.6).contains(&w.share) && finite(&w.at) && finite(&w.turn)));
        }
        if let Some(bare) = &self.bare {
            check("the empty hand must have 1 to 8 pieces", (1..=8).contains(&bare.pieces.len()));
            check("the empty hand's pieces must each name a texture", bare.pieces.iter().all(|p| !p.trim().is_empty()));
            check("the empty hand's pieces must be 0.3 to 1.5 widths apart", bare.spacing.is_finite() && (0.3..=1.5).contains(&bare.spacing));
        }
        let bare = self.bare.as_ref().map(Bare::kind);
        let mut tags = std::collections::BTreeSet::new();
        for (label, kind) in self.kinds.iter().map(|k| (k.tag.as_str(), k)).chain([("cube", &self.cube), ("sprite", &self.sprite)]).chain(bare.iter().map(|k| ("bare", k))) {
            let own = kind.moves;
            let all = self.moves_of(kind);
            let named = format!("`{label}`");
            check(&format!("{named} has an empty tag"), !kind.tag.is_empty());
            check(&format!("{named} is listed twice"), ["cube", "sprite", "bare"].contains(&label) || tags.insert(label.to_owned()));
            let p = &kind.pose;
            check(&format!("{named} has numbers that are not finite"), finite(&p.at) && finite(&p.turn) && finite(&[p.size]));
            check(&format!("{named} must be 0.05 to 1.5 blocks wide"), (0.05..=1.5).contains(&p.size));
            // Ahead of the eye and in reach of the view: a block or two at most.
            check(&format!("{named} must be 0.15 to 1.5 blocks forward"), (0.15..=1.5).contains(&p.at[2]));
            check(&format!("{named} must be within a block and a half right, left, up or down"), p.at[0].abs() <= 1.5 && p.at[1].abs() <= 1.5);
            for (name, stroke) in [("attack", own.attack), ("dig", own.dig), ("place", own.place), ("dodge", own.dodge), ("flinch", own.flinch)] {
                if let Some(s) = stroke {
                    check(&format!("{named}'s {name} stroke must take 0.05 to 3 seconds"), s.seconds.is_finite() && (0.05..=3.0).contains(&s.seconds));
                    check(&format!("{named}'s {name} stroke's wind-up must take 0 to 0.6 of it, in finite numbers"), s.wind.is_none_or(|w| (0.0..=0.6).contains(&w.share) && finite(&w.at) && finite(&w.turn)));
                }
            }
            for (name, at, turn) in [
                ("attack", all.attack.at, all.attack.turn),
                ("dig", all.dig.at, all.dig.turn),
                ("place", all.place.at, all.place.turn),
                ("guard", all.guard.at, all.guard.turn),
                ("eat", all.eat.at, all.eat.turn),
                ("dodge", all.dodge.at, all.dodge.turn),
                ("flinch", all.flinch.at, all.flinch.turn),
            ] {
                check(&format!("{named}'s {name} numbers must be finite"), finite(&at) && finite(&turn));
            }
        }
        problems
    }

    /// Kinds whose tag no item in `registry` has, which can never be held.
    pub fn unused(&self, registry: &BlockRegistry) -> Vec<String> {
        self.kinds
            .iter()
            .filter(|k| !registry.ids().any(|id| registry.def(id).has_tag(&k.tag)))
            .map(|k| format!("{FILE}: no item is tagged `{}`", k.tag))
            .collect()
    }
}

/// Merges the books of the packs (highest priority first), finishes and checks the result.
/// Anything wrong is logged and nothing is held in the hand; `registry` lets it say which
/// poses no item can use.
pub fn load(books: Vec<(String, Book)>, registry: &BlockRegistry) -> Handled {
    let mut merged = Book::default();
    let mut from = String::new();
    for (pack, book) in books.into_iter().rev() {
        merged.overlay(book, &mut |what| info!("texture pack {pack}: {FILE} replaces {what} of {}", if from.is_empty() { "an earlier pack" } else { &from }));
        from = pack;
    }
    match merged.finish() {
        Ok(handling) => {
            for note in handling.unused(registry) {
                warn!("{note}; leaving it out");
            }
            Handled(Some(handling))
        }
        Err(problems) => {
            for problem in problems {
                warn!("{problem}; no item is drawn in the hand");
            }
            Handled(None)
        }
    }
}

/// Seconds each frame moves the hand on whatever the frame took: the gate's, so a slow
/// machine shows the same swing (unset: the frame's own game seconds).
#[derive(Resource, Default)]
pub struct HeldClock {
    pub step: Option<f32>,
}

/// What the player is doing that moves the hand, this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Doing {
    /// What is held (none shows nothing).
    pub item: Option<BlockId>,
    /// Working at a block: the left button held with no creature in the way.
    pub digging: bool,
    /// A swing started this frame.
    pub swung: bool,
    /// Seconds from a swing's start to its blow, as the server judges it (the weapon class's
    /// wind-up): the stroke peaks then, however long its frames.
    pub connect: f32,
    /// A dodge began this frame, to the right (1) or the left (-1).
    pub dodged: Option<f32>,
    /// A blow was taken unguarded this frame.
    pub flinched: bool,
    /// A right click: placing, using, eating.
    pub using: bool,
    pub guarding: bool,
    pub eating: bool,
    /// Blocks a second along the ground.
    pub speed: f32,
    /// A blow that landed holds the swing still a moment (`fight_fx::HitStop`).
    pub frozen: bool,
}

/// The hand's motion from frame to frame.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct Motion {
    item: Option<BlockId>,
    /// Seconds since the item was taken up.
    raised: f32,
    /// Seconds into each stroke under way (the attack's with when it connects, the dodge's
    /// with its side).
    attack: Option<(f32, f32)>,
    dig: Option<f32>,
    place: Option<f32>,
    dodge: Option<(f32, f32)>,
    flinch: Option<f32>,
    /// How far each stance has come up, 0 to 1.
    guard: f32,
    eat: f32,
    /// Phases of the breath and the step, in turns.
    breath: f32,
    stride: f32,
    /// The speed, smoothed.
    speed: f32,
    /// The rhythm's seconds into the last swing, last frame.
    since_swing: f32,
    /// The swings at the air seen (`Fighting::whiffs`).
    whiffs: u32,
    /// Whether a dodge was under way last frame, and how many blows at the player had been
    /// heard.
    dashing: bool,
    hurt_seen: Option<u32>,
    /// The swing as last weighed: seconds into it, when it connects and its weight.
    swung: Option<(f32, f32, f32)>,
    /// The models the hand was drawn with last frame: the item's one, the empty hand's
    /// pieces, or none.
    drawn: usize,
    /// The light the hand has, as it follows the light where the player stands.
    light: Option<[f32; 2]>,
    /// The light measured where the player's eye is, the block it was measured at and when.
    lit: Option<(IVec3, [u8; 2], f32)>,
}

/// What the hand adds to its pose this frame, each 0 to 1 (or turns).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weights {
    pub raise: f32,
    pub attack: f32,
    /// How far back the attack's wind-up has drawn the hand.
    pub attack_wind: f32,
    pub dig: f32,
    pub place: f32,
    pub guard: f32,
    pub eat: f32,
    pub dodge: f32,
    /// The dodge's side: 1 to the right, -1 to the left.
    pub dodge_side: f32,
    pub flinch: f32,
    pub breath: f32,
    pub stride: f32,
    pub sway: f32,
}

/// A stroke's weight `progress` (0 to 1) of the way through: out and back, going out fast.
pub fn stroke_weight(progress: f32, skew: f32) -> f32 {
    (std::f32::consts::PI * progress.clamp(0.0, 1.0).powf(skew)).sin()
}

/// How far through a stroke (0 to 1) it peaks: past its wind-up, where the stroke proper is
/// fullest.
pub fn peak_of(stroke: &Stroke, skew: f32) -> f32 {
    let share = stroke.wind.map_or(0.0, |w| w.share);
    share + (1.0 - share) * 0.5f32.powf(1.0 / skew)
}

/// How far through `stroke` (0 to 1) it is `t` seconds after it started, peaking `connect`
/// seconds in (the blow), and how long it lasts: up to the peak in `connect`, and on from it
/// to rest in the stroke's own time after its peak. Without a connect (0), its own pace.
pub fn struck_progress(t: f32, connect: f32, stroke: &Stroke, skew: f32) -> (f32, f32) {
    let peak = peak_of(stroke, skew);
    let connect = if connect > 0.0 { connect } else { stroke.seconds * peak };
    let after = stroke.seconds * (1.0 - peak);
    let progress = if t < connect { peak * t / connect } else { peak + (1.0 - peak) * ((t - connect) / after.max(1e-3)).min(1.0) };
    (progress, connect + after)
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Motion {
    /// Advances the hand by `dt` seconds of what the player does.
    pub fn step(&mut self, doing: Doing, dt: f32, hand: &Hand, moves: &AllMoves) -> Weights {
        if doing.item != self.item {
            // The rhythm's reading goes on (`swing_began`): a click the frame after a switch
            // starts it over from where the switch left it, and is a swing.
            *self = Motion { item: doing.item, light: self.light, lit: self.lit, dashing: self.dashing, hurt_seen: self.hurt_seen, whiffs: self.whiffs, since_swing: self.since_swing, ..Motion::default() };
        }
        self.raised += dt;
        // Strokes run their course, and a held button starts the next.
        let run = |stroke: &mut Option<f32>, start: bool, seconds: f32, dt: f32| {
            if start && stroke.is_none() {
                *stroke = Some(0.0);
            }
            if let Some(t) = stroke {
                *t += dt;
                if *t >= seconds {
                    *stroke = None;
                }
            }
        };
        // A stroke that has just ended gets its last frame at rest: step first, weigh after.
        // A swing starts over at each click, timed to peak as its blow is judged.
        if doing.swung {
            self.attack = Some((0.0, doing.connect));
        }
        if let Some((t, connect)) = &mut self.attack {
            *t += if doing.frozen { 0.0 } else { dt };
            if *t >= struck_progress(*t, *connect, &moves.attack, hand.skew).1 {
                self.attack = None;
            }
        }
        run(&mut self.dig, doing.digging && !doing.swung, moves.dig.seconds, dt);
        run(&mut self.place, doing.using && !doing.guarding, moves.place.seconds, dt);
        if let Some(side) = doing.dodged {
            self.dodge = Some((0.0, side));
        }
        if let Some((t, _)) = &mut self.dodge {
            *t += dt;
            if *t >= moves.dodge.seconds {
                self.dodge = None;
            }
        }
        run(&mut self.flinch, doing.flinched, moves.flinch.seconds, dt);
        let toward = |value: &mut f32, on: bool| {
            let step = dt / hand.stance_seconds;
            *value = if on { (*value + step).min(1.0) } else { (*value - step).max(0.0) };
        };
        toward(&mut self.guard, doing.guarding);
        toward(&mut self.eat, doing.eating);
        let follow = 1.0 - (-8.0 * dt).exp();
        self.speed += (doing.speed - self.speed) * follow;
        self.breath = (self.breath + dt * hand.breath_rate).fract();
        self.stride = (self.stride + dt * self.speed * hand.steps_per_block * 0.5).fract();
        let weight = |stroke: Option<f32>, seconds: f32| stroke.map_or(0.0, |t| stroke_weight(t / seconds, hand.skew));
        let (attack_wind, attack) = self.attack.map_or((0.0, 0.0), |(t, connect)| wound_weights(struck_progress(t, connect, &moves.attack, hand.skew).0, moves.attack.wind, hand.skew));
        self.swung = self.attack.map(|(t, connect)| (t, connect, attack));
        Weights {
            raise: smoothstep(self.raised / hand.raise_seconds),
            attack,
            attack_wind,
            dig: weight(self.dig, moves.dig.seconds),
            place: weight(self.place, moves.place.seconds),
            guard: smoothstep(self.guard),
            eat: smoothstep(self.eat),
            dodge: self.dodge.map_or(0.0, |(t, _)| stroke_weight(t / moves.dodge.seconds, hand.skew)),
            dodge_side: self.dodge.map_or(1.0, |(_, side)| side),
            flinch: weight(self.flinch, moves.flinch.seconds),
            breath: self.breath * std::f32::consts::TAU,
            stride: self.stride * std::f32::consts::TAU,
            sway: (self.speed / hand.sway_speed).min(1.5),
        }
    }

    /// The swing under way as last weighed, if one is: seconds into it, the seconds in at
    /// which it connects (its blow is judged) and how far out the hand is (0 to 1).
    pub fn swing(&self) -> Option<(f32, f32, f32)> {
        self.swung
    }

    /// The models the hand was drawn with last frame.
    pub fn drawn(&self) -> usize {
        self.drawn
    }

    /// Whether a swing at the air began (`Fighting::whiff`), not for a switch of what is
    /// held.
    pub fn whiff_began(&mut self, whiffs: u32, switched: bool) -> bool {
        let began = !switched && whiffs != self.whiffs;
        self.whiffs = whiffs;
        began
    }

    /// Whether a swing began: the rhythm's seconds into its swing dropped (a click starts
    /// the meter again from 0), not for a switch of what is held.
    pub fn swing_began(&mut self, since: f32, switched: bool) -> bool {
        let began = !switched && since < self.since_swing - 1e-3;
        self.since_swing = since;
        began
    }

    /// Which way a dodge began this frame, if one did: 1 to the player's right, -1 to the
    /// left (straight back counts as right).
    pub fn dodge_began(&mut self, dash: Option<Vec3>, right: Vec3) -> Option<f32> {
        let began = dash.filter(|_| !self.dashing).map(|dir| if dir.dot(right) < -0.1 { -1.0 } else { 1.0 });
        self.dashing = dash.is_some();
        began
    }

    /// Whether a blow was taken unguarded since last frame: `count` blows heard in all, the
    /// latest `hurt`.
    pub fn flinch_began(&mut self, count: u32, latest: impl Iterator<Item = (u16, Defended)>) -> bool {
        let new = self.hurt_seen.map_or(0, |seen| count.wrapping_sub(seen) as usize);
        self.hurt_seen = Some(count);
        latest.take(new).any(|(damage, defended)| defended == Defended::Hit && damage > 0)
    }

    /// The light the hand has now, following `measured` (sky, block) where the player is.
    pub fn light(&mut self, measured: [u8; 2], dt: f32, rate: f32) -> [u8; 2] {
        let measured = measured.map(f32::from);
        let follow = 1.0 - (-rate * dt).exp();
        let now = match self.light {
            Some(light) => [0, 1].map(|i| light[i] + (measured[i] - light[i]) * follow),
            None => measured,
        };
        self.light = Some(now);
        now.map(|l| l.round().clamp(0.0, 15.0) as u8)
    }
}

/// The pose of the hand for `weights`: where its middle is (right, up and forward of the
/// eye), how it is turned from facing the eye (degrees) and its width.
pub fn compose(pose: &Pose, moves: &AllMoves, hand: &Hand, w: &Weights) -> (Vec3, Vec3, f32) {
    let (mut at, mut turn) = (Vec3::from(pose.at), Vec3::from(pose.turn));
    let mut add = |by_at: [f32; 3], by_turn: [f32; 3], weight: f32| {
        at += Vec3::from(by_at) * weight;
        turn += Vec3::from(by_turn) * weight;
    };
    add(moves.attack.at, moves.attack.turn, w.attack);
    if let Some(wind) = moves.attack.wind {
        add(wind.at, wind.turn, w.attack_wind);
    }
    add(moves.dig.at, moves.dig.turn, w.dig);
    add(moves.place.at, moves.place.turn, w.place);
    add(moves.guard.at, moves.guard.turn, w.guard);
    add(moves.eat.at, moves.eat.turn, w.eat);
    // A dodge to the left mirrors one to the right: across, and the yaw and roll.
    let side = w.dodge_side.signum();
    let (d, t) = (moves.dodge.at, moves.dodge.turn);
    add([d[0] * side, d[1], d[2]], [t[0], t[1] * side, t[2] * side], w.dodge);
    add(moves.flinch.at, moves.flinch.turn, w.flinch);
    at.y -= (1.0 - w.raise) * hand.lowered;
    at.y += w.breath.sin() * hand.breath;
    at.x += w.stride.sin() * hand.sway[0] * w.sway;
    at.y += (w.stride * 2.0).cos().abs() * hand.sway[1] * w.sway;
    (at, turn, pose.size)
}

/// The rotation vector of a turn given as pitch, yaw and roll in degrees.
pub fn turn_vector(degrees: Vec3) -> Vec3 {
    let [pitch, yaw, roll] = degrees.to_array().map(f32::to_radians);
    Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll).to_scaled_axis()
}

/// Puts the item in hand among the models to draw.
#[allow(clippy::too_many_arguments)]
fn show(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    target: Res<Target>,
    fighting: Res<Fighting>,
    launch: Res<Launch>,
    world: Res<ClientWorld>,
    pieces: Option<Res<TexturePieces>>,
    handled: Res<Handled>,
    mut motion: ResMut<Motion>,
    mut models: ResMut<ItemModels>,
    clock: Res<HeldClock>,
    (stop, kick): (Option<Res<crate::fight_fx::HitStop>>, Option<Res<crate::fight_fx::HandKick>>),
) {
    let Some(handling) = &handled.0 else { return };
    let now = time.elapsed_secs();
    let dt = clock.step.unwrap_or(time.delta_secs());
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let item = held.filter(|_| !player.dead && player.joined);
    let switched = item != motion.item;
    // How far into its last swing the player's rhythm is: a click starts it over.
    let combat = &launch.content.combat;
    let level = |skill| launch.content.skills.level(player.skills.xp(skill));
    let timing = combat.timing(&combat.wielded(held, level));
    let since = fighting.rhythm.meter(time.elapsed_secs_f64(), &timing) * timing.cooldown;
    let guarding = fighting.guard.is_some();
    let eating = target.eating_until > now;
    let whiffed = motion.whiff_began(fighting.whiffs, switched);
    let swung = (motion.swing_began(since, switched) || whiffed) && !guarding && !eating;
    let dodged = motion.dodge_began(fighting.dash.map(|(dir, ..)| dir), player.rotation() * Vec3::X);
    let flinched = motion.flinch_began(fighting.hurt_count, fighting.hurt.iter().rev().map(|&(_, damage, defended, _)| (damage, defended)));
    let locked = ui.cursor_locked && !player.dead;
    let doing = Doing {
        item,
        digging: locked && mouse.pressed(MouseButton::Left) && target.creature.is_none(),
        swung,
        connect: timing.windup,
        dodged,
        flinched,
        using: locked && mouse.pressed(MouseButton::Right),
        guarding,
        eating,
        speed: player.velocity.xz().length(),
        frozen: stop.is_some_and(|stop| stop.holds(now)),
    };
    // The kind comes first, for the moves its strokes time: the item's, or the empty
    // hand's (none while dead or not yet in the world).
    let eye_cell = player.eye().floor().as_ivec3();
    let bare = handling.bare.as_ref().filter(|_| item.is_none() && !player.dead && player.joined && !handling.bare_layers.is_empty());
    let bare_kind = bare.map(Bare::kind);
    let (kind, look) = match (item, &bare_kind) {
        (Some(item), _) => {
            let look = look_of(&world, pieces.as_deref(), item, eye_cell);
            let def = world.registry.def(item);
            (handling.kind_of(|tag| def.has_tag(tag), look.shape), Some(look))
        }
        (None, Some(kind)) => (kind, None),
        (None, None) => {
            motion.step(doing, dt, &handling.hand, &handling.moves);
            motion.drawn = 0;
            return;
        }
    };
    let moves = handling.moves_of(kind);
    let weights = motion.step(doing, dt, &handling.hand, &moves);
    let (mut at, turn, size) = compose(&kind.pose, &moves, &handling.hand, &weights);
    // A blow landing, or one taken on the guard, knocks the hand back toward the eye.
    if let Some(kick) = kick {
        at += kick.offset(now);
    }
    // Measured again as the player moves on and every so often where they stand: a torch
    // put down beside them or a roof built over them changes it.
    let measured = match motion.lit {
        Some((cell, light, at)) if cell == eye_cell && now - at < handling.hand.measure_seconds => light,
        _ => {
            let light = [sky_light(&world, eye_cell), block_light(&world, eye_cell)];
            motion.lit = Some((eye_cell, light, now));
            light
        }
    };
    let light = motion.light(measured, dt, handling.hand.light_rate);
    // First, not last: the renderer keeps the first `CAPACITY` models, and when drops fill
    // the list it is the furthest of them that go, not the hand.
    let (Some(look), None) = (look, bare) else {
        // The empty hand: the fist, then the arm back toward the eye. All of it is drawn at
        // one depth in front of the world, so each piece covers those before it: the
        // nearest goes last.
        let Some(bare) = bare else { return };
        let turned = turn_vector(turn);
        let arm = bare.pieces_at(at, turn, size).into_iter().zip(&handling.bare_layers).map(|(at, &layer)| ItemModel {
            center: Vec3::ZERO,
            phase: 0.0,
            offset: Vec3::ZERO,
            size,
            shape: Shape::Cube,
            layers: [layer; 6],
            tinted: 0,
            tint: [255; 3],
            light,
            held: Some(Held { at, turn: turned }),
            posed: None,
        });
        models.0.splice(0..0, arm);
        motion.drawn = handling.bare_layers.len().min(bare.pieces.len());
        return;
    };
    motion.drawn = 1;
    models.0.insert(0, ItemModel {
        center: Vec3::ZERO,
        phase: 0.0,
        offset: Vec3::ZERO,
        size,
        shape: look.shape,
        layers: look.layers,
        tinted: look.tinted,
        tint: look.tint,
        light,
        held: Some(Held { at, turn: turn_vector(turn) }),
        posed: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    fn shipped() -> Handling {
        let source = std::fs::read_to_string(assets().join("packs/default").join(FILE)).expect("the default pack has a held.ron");
        Book::parse(&source).unwrap().finish().unwrap_or_else(|problems| panic!("{problems:#?}"))
    }

    fn weights() -> Weights {
        Weights { raise: 1.0, ..Weights::default() }
    }

    #[test]
    fn the_default_pack_holds_every_weapon_and_tool_as_its_own_kind() {
        let handling = shipped();
        let content = voxmar_server::load_content(&assets()).unwrap();
        let blocks = &content.blocks;
        assert!(handling.unused(blocks).is_empty(), "{:?}", handling.unused(blocks));
        let (mut weapons, mut tools) = (0, 0);
        for id in blocks.ids().filter(|&id| blocks.holdable(id)) {
            let def = blocks.def(id);
            let look = if blocks.info(id).render == voxmar_engine::block::RenderKind::Plant { Shape::Sprite } else { Shape::Cube };
            let kind = handling.kind_of(|t| def.has_tag(t), look);
            for family in ["weapon", "tool"] {
                if def.has_tag(family) {
                    // A fist-sized fallback would hold a pickaxe like a pebble.
                    assert!(kind.tag != "sprite" && kind.tag != "cube", "{} is held as a plain {}", def.name, kind.tag);
                    if family == "weapon" { weapons += 1 } else { tools += 1 }
                }
            }
        }
        assert!(weapons >= 6 && tools >= 15, "{weapons} weapons and {tools} tools were held");
    }

    /// The middle and the four corners of an item's face in the view, on screen (-1 to 1 each
    /// way), at an aspect.
    fn face(at: Vec3, turn: Vec3, size: f32, aspect: f32) -> (Vec2, Vec<Vec2>) {
        let rotation = Quat::from_scaled_axis(turn_vector(turn));
        let f = 1.0 / (75f32.to_radians() / 2.0).tan();
        let project = |offset: Vec3| {
            let p = Vec3::new(at.x, at.y, -at.z) + rotation * offset * size;
            Vec2::new(p.x * f / aspect, p.y * f) / -p.z
        };
        (project(Vec3::ZERO), [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)].map(|(x, y)| project(Vec3::new(x, y, 0.0))).to_vec())
    }

    #[test]
    fn every_pose_stays_in_the_lower_right_of_the_view_at_rest_and_in_frame_through_every_move() {
        let handling = shipped();
        let bare = handling.bare.as_ref().map(Bare::kind);
        for aspect in [4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            for kind in handling.kinds.iter().chain([&handling.cube, &handling.sprite]).chain(&bare) {
                let moves = handling.moves_of(kind);
                let name = &kind.tag;
                let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, &weights());
                let (middle, rest) = face(at, turn, size, aspect);
                // The middle of it is in the lower right, and none of it in the way of the crosshair.
                assert!(middle.x > 0.15 && middle.y < -0.1, "{name} rests at {middle} in the view");
                assert!(middle.x < 0.85 && middle.y > -0.85, "{name} rests too near the screen's edge at aspect {aspect}: its middle is at {middle}");
                // No corner of its face comes near the crosshair at rest.
                assert!(rest.iter().all(|c| c.x > 0.05 || c.y < -0.05), "{name} covers the crosshair at rest: {rest:?}");
                // Every move, at its peak (the stroke's weight is 1 there), keeps its middle in view.
                for (move_name, w) in [
                    ("attack", Weights { attack: 1.0, ..weights() }),
                    ("attack's wind-up", Weights { attack_wind: 1.0, ..weights() }),
                    ("dig", Weights { dig: 1.0, ..weights() }),
                    ("place", Weights { place: 1.0, ..weights() }),
                    ("guard", Weights { guard: 1.0, ..weights() }),
                    ("eat", Weights { eat: 1.0, ..weights() }),
                    ("dodge right", Weights { dodge: 1.0, dodge_side: 1.0, ..weights() }),
                    ("dodge left", Weights { dodge: 1.0, dodge_side: -1.0, ..weights() }),
                    ("flinch", Weights { flinch: 1.0, ..weights() }),
                ] {
                    let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, &w);
                    let (middle, _) = face(at, turn, size, aspect);
                    assert!(at.z > 0.15, "{name}'s {move_name} goes behind the eye");
                    assert!(middle.x.abs() < 1.0 && middle.y.abs() < 1.0, "{name}'s {move_name} leaves the screen: its middle is at {middle}");
                    // The empty hand's arm runs back toward the eye and down out of the view,
                    // never past the eye.
                    if let Some(arm) = handling.bare.as_ref().filter(|_| kind.tag == "bare") {
                        let pieces = arm.pieces_at(at, turn, size);
                        assert!(pieces.iter().all(|p| p.z > 0.1), "the empty hand's {move_name} brings its arm past the eye: {pieces:?}");
                        assert!(pieces.windows(2).all(|p| p[1].z < p[0].z), "the empty hand's arm must run back toward the eye in its {move_name}: {pieces:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn an_item_just_taken_up_starts_below_the_view_and_comes_up() {
        let handling = shipped();
        let (hand, moves) = (&handling.hand, &handling.moves);
        let mut motion = Motion::default();
        let doing = Doing { item: Some(BlockId(5)), ..Doing::default() };
        let first = motion.step(doing, 0.0, hand, moves);
        assert_eq!(first.raise, 0.0);
        let (low, ..) = compose(&handling.sprite.pose, moves, hand, &first);
        assert!((low.y - (handling.sprite.pose.at[1] - hand.lowered)).abs() < 1e-3, "{low}");
        let up = motion.step(doing, hand.raise_seconds + 0.01, hand, moves);
        assert_eq!(up.raise, 1.0);
        // Switching starts it over, from below again.
        let other = motion.step(Doing { item: Some(BlockId(6)), ..Doing::default() }, 0.0, hand, moves);
        assert_eq!(other.raise, 0.0);
    }

    #[test]
    fn a_stroke_goes_out_and_back_and_a_held_button_starts_the_next() {
        assert_eq!(stroke_weight(0.0, 0.7), 0.0);
        assert!(stroke_weight(1.0, 0.7).abs() < 1e-5);
        // Out fast: the peak comes before the middle of the stroke.
        assert!(stroke_weight(0.37, 0.7) > 0.99 && stroke_weight(0.5, 0.7) < stroke_weight(0.37, 0.7));
        let handling = shipped();
        let (hand, moves) = (handling.hand, handling.moves);
        let mut motion = Motion::default();
        let digging = Doing { item: Some(BlockId(5)), digging: true, ..Doing::default() };
        let mut peaks = 0;
        let mut last = 0.0;
        let mut rising = false;
        // Two seconds of holding the button, in frames of 20 ms: a stroke every `seconds`.
        for _ in 0..100 {
            let w = motion.step(digging, 0.02, &hand, &moves).dig;
            if w < last && rising {
                peaks += 1;
            }
            rising = w > last;
            last = w;
        }
        let expected = (2.0 / moves.dig.seconds).floor() as i32;
        assert!((peaks - expected).abs() <= 1, "{peaks} strokes in 2 s of a {} s stroke", moves.dig.seconds);
        // Let go, and the stroke under way finishes and none follows.
        let rest = Doing { item: Some(BlockId(5)), ..Doing::default() };
        for _ in 0..100 {
            motion.step(rest, 0.02, &hand, &moves);
        }
        assert_eq!(motion.step(rest, 0.02, &hand, &moves).dig, 0.0);
    }

    /// Where a swing's attack weight is fullest, in seconds from the click, stepping `dt`.
    fn swing(handling: &Handling, kind: &Kind, connect: f32, dt: f32) -> Vec<(f32, f32)> {
        let moves = handling.moves_of(kind);
        let mut motion = Motion::default();
        let rest = Doing { item: Some(BlockId(5)), connect, ..Doing::default() };
        motion.step(rest, 1.0, &handling.hand, &moves);
        let mut t = 0.0;
        let mut seen = vec![(0.0, motion.step(Doing { swung: true, ..rest }, 0.0, &handling.hand, &moves).attack)];
        while t < 1.0 {
            t += dt;
            seen.push((t, motion.step(rest, dt, &handling.hand, &moves).attack));
        }
        seen
    }

    #[test]
    fn a_swing_peaks_as_its_blow_is_judged_however_long_the_frames() {
        let handling = shipped();
        let bare = handling.bare.as_ref().map(Bare::kind);
        assert!(bare.is_some(), "the default pack draws the empty hand");
        for kind in handling.kinds.iter().chain([&handling.cube, &handling.sprite]).chain(&bare) {
            for connect in [0.06, 0.08, 0.1, 0.15, 0.2] {
                let fine = swing(&handling, kind, connect, 1.0 / 480.0);
                let (peak, top) = fine.iter().copied().fold((0.0, 0.0), |best, (t, w)| if w > best.1 { (t, w) } else { best });
                assert!(top > 0.99, "{}'s swing never reaches its full stroke ({top})", kind.tag);
                assert!((peak - connect).abs() <= 1.0 / 240.0, "{}'s swing peaks at {peak:.3} s, not as its blow is judged at {connect} s", kind.tag);
                // It comes back to rest.
                assert_eq!(fine.last().unwrap().1, 0.0, "{}'s swing never comes back", kind.tag);
                // Frames of 20 fps see the same swing at the same moments as frames of 480.
                for (t, w) in swing(&handling, kind, connect, 1.0 / 20.0) {
                    let (_, near) = fine.iter().copied().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).unwrap();
                    assert!((w - near).abs() < 0.03, "{}'s swing at {t:.2} s is {w} at 20 fps and {near} at 480", kind.tag);
                }
            }
        }
        // A blow held still by a hit landing holds the stroke at its peak.
        let kind = &handling.kinds[0];
        let moves = handling.moves_of(kind);
        let mut motion = Motion::default();
        let doing = Doing { item: Some(BlockId(5)), connect: 0.1, ..Doing::default() };
        motion.step(Doing { swung: true, ..doing }, 0.0, &handling.hand, &moves);
        let at_peak = motion.step(doing, 0.1, &handling.hand, &moves).attack;
        assert_eq!(motion.step(Doing { frozen: true, ..doing }, 0.05, &handling.hand, &moves).attack, at_peak);
    }

    #[test]
    fn a_dodge_leaves_the_hand_behind_on_its_side_and_a_blow_taken_knocks_it() {
        let handling = shipped();
        let (hand, moves) = (handling.hand, handling.moves);
        let kind = &handling.kinds[0];
        let mut motion = Motion::default();
        let right = Vec3::X;
        assert_eq!(motion.dodge_began(Some(Vec3::X), right), Some(1.0));
        assert_eq!(motion.dodge_began(Some(Vec3::X), right), None, "once, as it begins");
        assert_eq!(motion.dodge_began(None, right), None);
        assert_eq!(motion.dodge_began(Some(-Vec3::X), right), Some(-1.0));
        motion.dodge_began(None, right);
        assert_eq!(motion.dodge_began(Some(Vec3::Z), right), Some(1.0), "straight back counts as right");
        // The two sides mirror each other across the view.
        let rest = Doing { item: Some(BlockId(5)), ..Doing::default() };
        let mut left = Motion::default();
        let mut both = [(&mut motion, 1.0), (&mut left, -1.0)].map(|(m, side)| {
            *m = Motion::default();
            m.step(Doing { dodged: Some(side), ..rest }, 0.0, &hand, &moves);
            let w = m.step(rest, moves.dodge.seconds * 0.35, &hand, &moves);
            assert!(w.dodge > 0.9, "the dodge's hand is at {}", w.dodge);
            compose(&kind.pose, &moves, &hand, &w).0
        });
        let rest_at = Vec3::from(kind.pose.at);
        both.iter_mut().for_each(|a| *a -= rest_at);
        assert!((both[0].x + both[1].x).abs() < 0.02 && both[0].x.abs() > 0.05, "{both:?}");
        // A blow taken unguarded knocks the hand; one blocked, or the first heard, does not.
        let mut motion = Motion::default();
        assert!(!motion.flinch_began(3, [(4, Defended::Hit)].into_iter()), "blows from before are not news");
        assert!(motion.flinch_began(4, [(4, Defended::Hit)].into_iter()));
        assert!(!motion.flinch_began(5, [(0, Defended::Perfect)].into_iter()));
        assert!(!motion.flinch_began(5, [(4, Defended::Hit)].into_iter()), "no new blow");
        let w = motion.step(Doing { flinched: true, ..rest }, 0.05, &hand, &moves);
        assert!(w.flinch > 0.3, "{}", w.flinch);
    }

    #[test]
    fn a_swing_is_seen_when_the_rhythm_starts_over_and_not_on_a_switch() {
        let mut motion = Motion::default();
        // At rest the meter reads a whole cooldown; a click makes it 0; then it grows.
        assert!(!motion.swing_began(1.0, false));
        assert!(motion.swing_began(0.0, false));
        assert!(!motion.swing_began(0.016, false));
        assert!(!motion.swing_began(0.5, false));
        // A switch of what is held restarts the rhythm too, which is no swing.
        assert!(!motion.swing_began(0.0, true));
    }

    /// A whiff is seen once, and not again when what is held changes after it. Before: the
    /// switch made the hand anew with no whiffs seen, and the next frame swung the new item.
    #[test]
    fn a_whiff_is_seen_once_and_not_again_after_a_switch() {
        let handling = shipped();
        let (hand, moves) = (handling.hand, handling.moves);
        let mut motion = Motion::default();
        assert!(!motion.whiff_began(0, false));
        assert!(motion.whiff_began(1, false));
        assert!(!motion.whiff_began(1, false));
        // A switch the frame it is seen, then the hand made anew for the new item.
        assert!(!motion.whiff_began(1, true));
        motion.step(Doing { item: Some(BlockId(5)), ..Doing::default() }, 0.05, &hand, &moves);
        assert!(!motion.whiff_began(1, false), "the switch was taken for a whiff");
        assert!(motion.whiff_began(2, false));
    }

    /// A click the frame after what is held changed is a swing, and the hand plays it: the
    /// switch's fresh hand once forgot the rhythm's last reading, so a meter starting over
    /// from full read as no drop at all (the gate's watched swing at the bear, pressed the
    /// frame the sword came into hand, never played).
    #[test]
    fn a_swing_the_frame_after_a_switch_is_seen_and_played() {
        let handling = shipped();
        let (hand, moves) = (handling.hand, handling.moves);
        let mut motion = Motion::default();
        let empty = Doing::default();
        assert!(!motion.swing_began(0.6, false));
        motion.step(empty, 1.0 / 60.0, &hand, &moves);
        // The sword comes into hand with the meter full: a switch, no swing.
        let sword = Doing { item: Some(BlockId(5)), ..Doing::default() };
        assert!(!motion.swing_began(0.6, true));
        motion.step(sword, 1.0 / 60.0, &hand, &moves);
        // The very next frame the click starts the meter over.
        assert!(motion.swing_began(0.0, false), "the swing the frame after the switch was not seen");
        motion.step(Doing { swung: true, connect: 0.1, ..sword }, 1.0 / 60.0, &hand, &moves);
        assert!(motion.swing().is_some(), "the hand did not play the swing");
    }

    #[test]
    fn the_stances_ease_in_and_out() {
        let handling = shipped();
        let (hand, moves) = (handling.hand, handling.moves);
        let mut motion = Motion::default();
        let guarding = Doing { item: Some(BlockId(5)), guarding: true, ..Doing::default() };
        assert_eq!(motion.step(guarding, hand.stance_seconds / 2.0, &hand, &moves).guard, 0.5);
        assert_eq!(motion.step(guarding, hand.stance_seconds, &hand, &moves).guard, 1.0);
        let down = motion.step(Doing { guarding: false, ..guarding }, hand.stance_seconds / 2.0, &hand, &moves);
        assert_eq!(down.guard, 0.5);
        // A guard up does not dip with a right click.
        let mut motion = Motion::default();
        let both = Doing { using: true, ..guarding };
        assert_eq!(motion.step(both, 0.05, &hand, &moves).place, 0.0);
    }

    #[test]
    fn the_light_follows_where_the_player_stands() {
        let mut motion = Motion::default();
        assert_eq!(motion.light([15, 0], 0.016, 8.0), [15, 0]);
        // Into a cave: it dims over a moment, not at once, and settles at the cave's light.
        let first = motion.light([0, 0], 0.016, 8.0);
        assert!(first[0] < 15 && first[0] > 10, "{first:?}");
        for _ in 0..200 {
            motion.light([0, 0], 0.016, 8.0);
        }
        assert_eq!(motion.light([0, 0], 0.016, 8.0), [0, 0]);
    }

    #[test]
    fn a_later_pack_replaces_entries_one_by_one_and_says_so() {
        let base = shipped();
        let mut book = Book::parse(&std::fs::read_to_string(assets().join("packs/default").join(FILE)).unwrap()).unwrap();
        let later = Book::parse(
            r#"(
                moves: (dig: Some((at: (0.0, 0.0, 0.0), turn: (-10.0, 0.0, 0.0), seconds: 0.5))),
                kinds: [
                    (tag: "sword", pose: (at: (0.3, -0.3, 0.6), turn: (0.0, 0.0, 45.0), size: 0.7)),
                    (tag: "wand", pose: (at: (0.3, -0.3, 0.6), turn: (0.0, 0.0, 45.0), size: 0.7)),
                ],
            )"#,
        )
        .unwrap();
        let mut notes = Vec::new();
        book.overlay(later, &mut |what| notes.push(what));
        assert_eq!(notes, ["the dig stroke", "the pose of sword"]);
        let handling = book.finish().unwrap();
        assert_eq!(handling.moves.dig.seconds, 0.5);
        assert_eq!(handling.moves.attack, base.moves.attack);
        assert_eq!(handling.kinds.iter().find(|k| k.tag == "sword").unwrap().pose.size, 0.7);
        // A new kind goes first, so it wins; nothing was lost.
        assert_eq!(handling.kinds[0].tag, "wand");
        assert_eq!(handling.kinds.len(), base.kinds.len() + 1);
    }

    #[test]
    fn a_book_that_is_not_whole_or_sensible_says_what_is_wrong() {
        let wrong = Book::parse("(kinds: [])").unwrap().finish().unwrap_err();
        assert!(wrong.iter().any(|p| p.contains("hand")) && wrong.iter().any(|p| p.contains("cube")), "{wrong:?}");
        assert!(wrong.iter().any(|p| p.contains("dodge stroke")) && wrong.iter().any(|p| p.contains("flinch stroke")), "{wrong:?}");
        let source = std::fs::read_to_string(assets().join("packs/default").join(FILE)).unwrap();
        let book = Book::parse(&source).unwrap();
        let changes: [(fn(&mut Book), &str); 11] = [
            (|b: &mut Book| b.kinds[0].pose.size = 0.0, "wide"),
            (|b: &mut Book| b.kinds[0].pose.at[2] = -0.5, "forward"),
            (|b: &mut Book| b.kinds[0].pose.at[0] = f32::NAN, "finite"),
            (|b: &mut Book| b.moves.attack.as_mut().unwrap().seconds = 0.0, "seconds"),
            (|b: &mut Book| b.hand.as_mut().unwrap().raise_seconds = 0.0, "raise_seconds"),
            (|b: &mut Book| b.kinds[1].tag = b.kinds[0].tag.clone(), "twice"),
            (|b: &mut Book| b.moves.dodge.as_mut().unwrap().seconds = 9.0, "dodge stroke must take"),
            (|b: &mut Book| b.bare.as_mut().unwrap().pieces.clear(), "1 to 8 pieces"),
            (|b: &mut Book| b.bare.as_mut().unwrap().pieces[0] = " ".into(), "each name a texture"),
            (|b: &mut Book| b.bare.as_mut().unwrap().spacing = 3.0, "widths apart"),
            (|b: &mut Book| b.bare.as_mut().unwrap().pose.at[2] = 0.0, "`bare` must be 0.15 to 1.5 blocks forward"),
        ];
        for (change, wanted) in changes {
            let mut broken = book.clone();
            change(&mut broken);
            let problems = broken.finish().unwrap_err();
            assert!(problems.iter().any(|p| p.contains(wanted)), "wanted `{wanted}` in {problems:?}");
        }
        assert!(Book::parse("(hand: 1)").is_err() && Book::parse("(nonsense: 1)").is_err());
    }

    /// A flat rendering of an item's icon `image` held at `at`, turned `turn`, `size` wide, in
    /// a view `w` by `h` (75 degrees high) over sky and grass with the crosshair: RGB bytes.
    fn flat(image: Option<&voxmar_texgen::Image>, at: Vec3, turn: Vec3, size: f32, w: usize, h: usize) -> Vec<u8> {
        let f = 1.0 / (75f32.to_radians() / 2.0).tan();
        let rotation = Quat::from_scaled_axis(turn_vector(turn));
        let (centre, normal) = (Vec3::new(at.x, at.y, -at.z), rotation * Vec3::Z);
        let mut pixels = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / h as f32 * 2.0);
                // Sky above the horizon, grass below.
                let mut rgb = if ny > -0.15 { [150u8, 195, 240] } else { [96, 150, 70] };
                let arm = (h / 70).max(2);
                if x.abs_diff(w / 2) < arm && y == h / 2 || y.abs_diff(h / 2) < arm && x == w / 2 {
                    rgb = [255, 255, 255];
                }
                let direction = Vec3::new(nx * (w as f32 / h as f32) / f, ny / f, -1.0);
                let t = centre.dot(normal) / direction.dot(normal);
                if let (Some(image), true) = (image, t > 0.0) {
                    let local = rotation.inverse() * (direction * t - centre) / size + Vec3::splat(0.5);
                    if (0.0..1.0).contains(&local.x) && (0.0..1.0).contains(&local.y) {
                        let (tx, ty) = ((local.x * image.width as f32) as u32, ((1.0 - local.y) * image.height as f32) as u32);
                        let [r, g, b, a] = image.get(tx, ty);
                        if a >= 128 {
                            rgb = [r, g, b];
                        }
                    }
                }
                pixels[(y * w + x) * 3..][..3].copy_from_slice(&rgb);
            }
        }
        pixels
    }

    /// A rendering of cubes held in the view, each at `at` turned `turn`, `size` wide, with
    /// its texture on every face, over sky and grass with the crosshair: RGB bytes. Drawn as
    /// the shader draws held models, all at one depth, so each covers those before it.
    fn boxes(cubes: &[(Vec3, Vec3, f32, Option<&voxmar_texgen::Image>)], w: usize, h: usize) -> Vec<u8> {
        let f = 1.0 / (75f32.to_radians() / 2.0).tan();
        let mut pixels = flat(None, Vec3::Z, Vec3::ZERO, 0.0, w, h);
        for &(at, turn, size, image) in cubes {
            let rotation = Quat::from_scaled_axis(turn_vector(turn));
            let inverse = rotation.inverse();
            let centre = Vec3::new(at.x, at.y, -at.z);
            for y in 0..h {
                for x in 0..w {
                    let (nx, ny) = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / h as f32 * 2.0);
                    let direction = Vec3::new(nx * (w as f32 / h as f32) / f, ny / f, -1.0);
                    // Into the cube's own unit box, centred on 0.
                    let (o, d) = (inverse * -centre / size, inverse * direction / size);
                    let (mut near, mut far, mut axis) = (f32::MIN, f32::MAX, 0);
                    for a in 0..3 {
                        if d[a].abs() < 1e-9 {
                            if o[a].abs() > 0.5 {
                                far = f32::MIN;
                            }
                            continue;
                        }
                        let (t0, t1) = ((-0.5 - o[a]) / d[a], (0.5 - o[a]) / d[a]);
                        let (t0, t1) = (t0.min(t1), t0.max(t1));
                        if t0 > near {
                            (near, axis) = (t0, a);
                        }
                        far = far.min(t1);
                    }
                    if near > far || near <= 0.0 {
                        continue;
                    }
                    let hit = o + d * near + Vec3::splat(0.5);
                    let (u, v) = match axis {
                        0 => (hit.z, hit.y),
                        1 => (hit.x, hit.z),
                        _ => (hit.x, hit.y),
                    };
                    let mut normal = Vec3::ZERO;
                    normal[axis] = -d[axis].signum();
                    let n = rotation * normal;
                    // The shader's light by the face's way: tops brightest, bottoms darkest.
                    let shade = n.x * n.x * 0.62 + n.z * n.z * 0.8 + n.y * n.y * if n.y > 0.0 { 1.0 } else { 0.5 };
                    let rgb = match image {
                        Some(image) => {
                            let (tx, ty) = (((u * image.width as f32) as u32).min(image.width - 1), (((1.0 - v) * image.height as f32) as u32).min(image.height - 1));
                            let [r, g, b, _] = image.get(tx, ty);
                            [r, g, b]
                        }
                        None => [200, 150, 110],
                    };
                    pixels[(y * w + x) * 3..][..3].copy_from_slice(&rgb.map(|c| (c as f32 * shade).min(255.0) as u8));
                }
            }
        }
        pixels
    }

    /// The empty hand frame by frame (`bare.png`), as `held_swing_strip`: a punch with the
    /// frame its blow is judged in red and its peak in yellow, a jab at a block, a use, the
    /// guard raised (with a shield worn), a dodge, a blow taken and the hand coming up.
    #[test]
    #[ignore]
    fn bare_hand_strip() {
        let Some(out) = std::env::var_os("VOXMAR_HELD_PREVIEW") else { return };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let handling = shipped();
        let content = voxmar_server::load_content(&assets()).unwrap();
        let library = voxmar_texgen::Library::load(&assets().join("packs/default")).unwrap();
        let bare = handling.bare.clone().unwrap();
        let images: Vec<Option<voxmar_texgen::Image>> = bare.pieces.iter().map(|p| library.render(p).and_then(Result::ok)).collect();
        let kind = bare.kind();
        let moves = handling.moves_of(&kind);
        let windup = content.combat.timing(&content.combat.wielded(None, |_| 1)).windup;
        const FRAMES: usize = 14;
        const STEP: f32 = 1.0 / 30.0;
        let (w, h) = (274usize, 154usize);
        let rows = ["punch", "dig", "use", "guard", "dodge right", "flinch", "raise"];
        let mut sheet = image::RgbImage::new((FRAMES * w) as u32, (rows.len() * h) as u32);
        for (row, what) in rows.iter().enumerate() {
            let mut motion = Motion::default();
            let rest = Doing { item: None, connect: windup, ..Doing::default() };
            if *what != "raise" {
                motion.step(rest, 1.0, &handling.hand, &moves);
            } else {
                motion.step(Doing { item: Some(BlockId(5)), ..rest }, 1.0, &handling.hand, &moves);
            }
            let mut frames = Vec::new();
            for frame in 0..FRAMES {
                let doing = match *what {
                    "punch" => Doing { swung: frame == 0, ..rest },
                    "dig" => Doing { digging: true, ..rest },
                    "use" => Doing { using: frame < 2, ..rest },
                    "guard" => Doing { guarding: true, ..rest },
                    "dodge right" => Doing { dodged: (frame == 0).then_some(1.0), ..rest },
                    "flinch" => Doing { flinched: frame == 0, ..rest },
                    _ => rest,
                };
                frames.push(motion.step(doing, if frame == 0 { 0.0 } else { STEP }, &handling.hand, &moves));
            }
            let punching = *what == "punch";
            let peak = if punching { (0..FRAMES).max_by(|&a, &b| frames[a].attack.total_cmp(&frames[b].attack)).unwrap() } else { FRAMES };
            let judged = if punching { (windup / STEP).round() as usize } else { FRAMES };
            println!("row {row}: bare {what}: wind-up {windup:.2} s (frame {judged}), stroke peaks at frame {peak} ({:.2} s)", peak as f32 * STEP);
            for (frame, weights) in frames.iter().enumerate() {
                let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, weights);
                let cubes: Vec<_> = bare.pieces_at(at, turn, size).into_iter().zip(&images).map(|(at, image)| (at, turn, size, image.as_ref())).collect();
                let pixels = boxes(&cubes, w, h);
                let (ox, oy) = (frame * w, row * h);
                for y in 0..h {
                    for x in 0..w {
                        let p = &pixels[(y * w + x) * 3..][..3];
                        let edge = x < 4 || y < 4 || x >= w - 4 || y >= h - 4;
                        let inner = (4..8).contains(&x) || (4..8).contains(&y) || (w - 8..w - 4).contains(&x) || (h - 8..h - 4).contains(&y);
                        let rgb = match (frame == judged, frame == peak) {
                            (true, _) if edge => [230, 40, 40],
                            (_, true) if edge || inner && frame == judged => [250, 210, 40],
                            _ if x == 0 || y == 0 => [30, 30, 30],
                            _ => [p[0], p[1], p[2]],
                        };
                        sheet.put_pixel((ox + x) as u32, (oy + y) as u32, image::Rgb(rgb));
                    }
                }
            }
        }
        sheet.save(out.join("bare.png")).unwrap();
    }

    /// Draws the hand as the player would see it, for looking at: `cargo test -p voxmar_client
    /// held_preview -- --ignored` with `VOXMAR_HELD_PREVIEW` the folder to write to. A flat
    /// sprite through the very pose maths, as the shader places it (no thickness).
    #[test]
    #[ignore]
    fn held_preview() {
        let Some(out) = std::env::var_os("VOXMAR_HELD_PREVIEW") else { return };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let handling = shipped();
        let library = voxmar_texgen::Library::load(&assets().join("packs/default")).unwrap();
        let (w, h) = (480usize, 270usize);
        let sheet: Vec<(&str, &str, Vec<(&str, Weights)>)> = vec![
            ("iron_sword", "sword", vec![
                ("rest", weights()),
                ("attack", Weights { attack: 1.0, ..weights() }),
                ("dig", Weights { dig: 1.0, ..weights() }),
                ("place", Weights { place: 1.0, ..weights() }),
                ("guard", Weights { guard: 1.0, ..weights() }),
                ("eat", Weights { eat: 1.0, ..weights() }),
            ]),
            ("stone_pickaxe", "pickaxe", vec![("rest", weights()), ("dig", Weights { dig: 1.0, ..weights() })]),
            ("fang_dagger", "dagger", vec![("rest", weights()), ("attack", Weights { attack: 1.0, ..weights() })]),
            ("bronze_spear", "spear", vec![("rest", weights()), ("attack", Weights { attack: 1.0, ..weights() })]),
            ("stone_hoe", "hoe", vec![("rest", weights())]),
            ("fishing_rod", "rod", vec![("rest", weights())]),
            ("torch_lit", "light", vec![("rest", weights())]),
            ("cooked_small_game", "edible", vec![("rest", weights()), ("eat", Weights { eat: 1.0, ..weights() })]),
        ];
        let panels: Vec<(String, Vec<u8>)> = sheet
            .iter()
            .flat_map(|(texture, tag, poses)| {
                let image = library.render(&format!("block/{texture}")).and_then(Result::ok);
                let kind = handling.kinds.iter().find(|k| k.tag == *tag).unwrap_or(&handling.sprite);
                let moves = handling.moves_of(kind);
                poses.iter().map(|(label, weights)| {
                    let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, weights);
                    let pixels = flat(image.as_ref(), at, turn, size, w, h);
                    (format!("{texture}-{label}"), pixels)
                })
                .collect::<Vec<_>>()
            })
            .collect();
        // A contact sheet, four panels across.
        let (columns, rows) = (4usize, panels.len().div_ceil(4));
        let mut sheet = image::RgbImage::new((columns * w) as u32, (rows * h) as u32);
        for (i, (name, pixels)) in panels.iter().enumerate() {
            let (ox, oy) = ((i % columns) * w, (i / columns) * h);
            for y in 0..h {
                for x in 0..w {
                    let p = &pixels[(y * w + x) * 3..][..3];
                    sheet.put_pixel((ox + x) as u32, (oy + y) as u32, image::Rgb([p[0], p[1], p[2]]));
                }
            }
            println!("panel {i}: {name}");
        }
        sheet.save(out.join("held.png")).unwrap();
    }
    /// Frames of the swing of each weapon kind, 1/30 s apart from the click, as a strip a row
    /// each, for looking at: `VOXMAR_HELD_PREVIEW=DIR cargo test -p voxmar_client
    /// held_swing_strip -- --ignored` writes `swings.png`, 3840 pixels across. The frame the
    /// server judges the blow (the class's wind-up) is framed in red, and the frame the stroke
    /// peaks in yellow; both, when they are the same frame.
    #[test]
    #[ignore]
    fn held_swing_strip() {
        let Some(out) = std::env::var_os("VOXMAR_HELD_PREVIEW") else { return };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let handling = shipped();
        let content = voxmar_server::load_content(&assets()).unwrap();
        let library = voxmar_texgen::Library::load(&assets().join("packs/default")).unwrap();
        const FRAMES: usize = 14;
        const STEP: f32 = 1.0 / 30.0;
        let (w, h) = (274usize, 154usize);
        let mut rows: Vec<(&str, &str, &str)> = ["sword", "dagger", "spear", "mace", "axe", "bow"].into_iter().zip(["iron_sword", "fang_dagger", "bronze_spear", "bone_club", "stone_axe", "short_bow"]).map(|(tag, texture)| (texture, tag, "swing")).collect();
        rows.extend([("iron_sword", "sword", "guard"), ("iron_sword", "sword", "dodge right"), ("iron_sword", "sword", "dodge left"), ("iron_sword", "sword", "flinch")]);
        let mut sheet = image::RgbImage::new((FRAMES * w) as u32, (rows.len() * h) as u32);
        for (row, (texture, tag, what)) in rows.iter().enumerate() {
            let item = content.blocks.id(&format!("base:{texture}")).unwrap();
            let weapon = content.combat.wielded(Some(item), |_| 99);
            let windup = content.combat.timing(&weapon).windup;
            let image = library.render(&format!("block/{texture}")).and_then(Result::ok);
            let kind = handling.kinds.iter().find(|k| k.tag == *tag).unwrap();
            let moves = handling.moves_of(kind);
            let mut motion = Motion::default();
            let rest = Doing { item: Some(item), connect: windup, ..Doing::default() };
            motion.step(rest, 1.0, &handling.hand, &moves);
            let mut frames = Vec::new();
            for frame in 0..FRAMES {
                let doing = match *what {
                    "swing" => Doing { swung: frame == 0, ..rest },
                    "guard" => Doing { guarding: true, ..rest },
                    "dodge right" => Doing { dodged: (frame == 0).then_some(1.0), ..rest },
                    "dodge left" => Doing { dodged: (frame == 0).then_some(-1.0), ..rest },
                    _ => Doing { flinched: frame == 0, ..rest },
                };
                let weights = motion.step(doing, if frame == 0 { 0.0 } else { STEP }, &handling.hand, &moves);
                frames.push(weights);
            }
            let swinging = *what == "swing";
            let peak = if swinging { (0..FRAMES).max_by(|&a, &b| frames[a].attack.total_cmp(&frames[b].attack)).unwrap() } else { FRAMES };
            let judged = if swinging { (windup / STEP).round() as usize } else { FRAMES };
            println!("row {row}: {tag} {what}: wind-up {windup:.2} s (frame {judged}), stroke peaks at frame {peak} ({:.2} s)", peak as f32 * STEP);
            for (frame, weights) in frames.iter().enumerate() {
                let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, weights);
                let pixels = flat(image.as_ref(), at, turn, size, w, h);
                let (ox, oy) = (frame * w, row * h);
                for y in 0..h {
                    for x in 0..w {
                        let p = &pixels[(y * w + x) * 3..][..3];
                        let edge = x < 4 || y < 4 || x >= w - 4 || y >= h - 4;
                        let inner = (4..8).contains(&x) || (4..8).contains(&y) || (w - 8..w - 4).contains(&x) || (h - 8..h - 4).contains(&y);
                        let rgb = match (frame == judged, frame == peak) {
                            (true, _) if edge => [230, 40, 40],
                            (_, true) if edge || inner && frame == judged => [250, 210, 40],
                            _ if x == 0 || y == 0 => [30, 30, 30],
                            _ => [p[0], p[1], p[2]],
                        };
                        sheet.put_pixel((ox + x) as u32, (oy + y) as u32, image::Rgb(rgb));
                    }
                }
            }
        }
        sheet.save(out.join("swings.png")).unwrap();
    }
}
