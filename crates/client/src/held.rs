//! The item in the player's hand, seen from their own eyes: a tool or a weapon, a block as a
//! small cube, any other item as a thin sprite given thickness, low in the right of the
//! view. It bobs as the player breathes and walks, comes up from below when the player
//! switches to it, and swings when they dig, strike or use something, timed by the same
//! things that time those (the mining in progress, the fight's rhythm, the right button).
//! The empty hand shows nothing.
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
            .add_systems(Update, show.after(crate::drops::show).run_if(in_state(Screen::InGame)))
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
/// `seconds` from start to rest.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub at: [f32; 3],
    pub turn: [f32; 3],
    pub seconds: f32,
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
        put(&mut self.cube, later.cube, "the pose of a block", note);
        put(&mut self.sprite, later.sprite, "the pose of any other item", note);
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
        need("pose for a block (cube)", self.cube.is_some());
        need("pose for other items (sprite)", self.sprite.is_some());
        let (Some(hand), Some(attack), Some(dig), Some(place), Some(guard), Some(eat), Some(cube), Some(sprite)) =
            (self.hand, self.moves.attack, self.moves.dig, self.moves.place, self.moves.guard, self.moves.eat, self.cube, self.sprite)
        else {
            return Err(problems);
        };
        let handling = Handling { hand, moves: AllMoves { attack, dig, place, guard, eat }, kinds: self.kinds, cube, sprite };
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
}

/// How things are held, whole and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Handling {
    pub hand: Hand,
    pub moves: AllMoves,
    pub kinds: Vec<Kind>,
    pub cube: Kind,
    pub sprite: Kind,
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
        for (name, stroke) in [("attack", self.moves.attack), ("dig", self.moves.dig), ("place", self.moves.place)] {
            check(&format!("the {name} stroke must take 0.05 to 3 seconds"), stroke.seconds.is_finite() && (0.05..=3.0).contains(&stroke.seconds));
        }
        let mut tags = std::collections::BTreeSet::new();
        for (label, kind) in self.kinds.iter().map(|k| (k.tag.as_str(), k)).chain([("cube", &self.cube), ("sprite", &self.sprite)]) {
            let own = kind.moves;
            let all = self.moves_of(kind);
            let named = format!("`{label}`");
            check(&format!("{named} has an empty tag"), !kind.tag.is_empty());
            check(&format!("{named} is listed twice"), label == "cube" || label == "sprite" || tags.insert(label.to_owned()));
            let p = &kind.pose;
            check(&format!("{named} has numbers that are not finite"), finite(&p.at) && finite(&p.turn) && finite(&[p.size]));
            check(&format!("{named} must be 0.05 to 1.5 blocks wide"), (0.05..=1.5).contains(&p.size));
            // Ahead of the eye and in reach of the view: a block or two at most.
            check(&format!("{named} must be 0.15 to 1.5 blocks forward"), (0.15..=1.5).contains(&p.at[2]));
            check(&format!("{named} must be within a block and a half right, left, up or down"), p.at[0].abs() <= 1.5 && p.at[1].abs() <= 1.5);
            for (name, stroke) in [("attack", own.attack), ("dig", own.dig), ("place", own.place)] {
                if let Some(s) = stroke {
                    check(&format!("{named}'s {name} stroke must take 0.05 to 3 seconds"), s.seconds.is_finite() && (0.05..=3.0).contains(&s.seconds));
                }
            }
            for (name, at, turn) in [
                ("attack", all.attack.at, all.attack.turn),
                ("dig", all.dig.at, all.dig.turn),
                ("place", all.place.at, all.place.turn),
                ("guard", all.guard.at, all.guard.turn),
                ("eat", all.eat.at, all.eat.turn),
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

/// What the player is doing that moves the hand, this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Doing {
    /// What is held (none shows nothing).
    pub item: Option<BlockId>,
    /// Working at a block: the left button held with no creature in the way.
    pub digging: bool,
    /// A swing started this frame.
    pub swung: bool,
    /// A right click: placing, using, eating.
    pub using: bool,
    pub guarding: bool,
    pub eating: bool,
    /// Blocks a second along the ground.
    pub speed: f32,
}

/// The hand's motion from frame to frame.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct Motion {
    item: Option<BlockId>,
    /// Seconds since the item was taken up.
    raised: f32,
    /// Seconds into each stroke under way.
    attack: Option<f32>,
    dig: Option<f32>,
    place: Option<f32>,
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
    pub dig: f32,
    pub place: f32,
    pub guard: f32,
    pub eat: f32,
    pub breath: f32,
    pub stride: f32,
    pub sway: f32,
}

/// A stroke's weight `progress` (0 to 1) of the way through: out and back, going out fast.
pub fn stroke_weight(progress: f32, skew: f32) -> f32 {
    (std::f32::consts::PI * progress.clamp(0.0, 1.0).powf(skew)).sin()
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Motion {
    /// Advances the hand by `dt` seconds of what the player does.
    pub fn step(&mut self, doing: Doing, dt: f32, hand: &Hand, moves: &AllMoves) -> Weights {
        if doing.item != self.item {
            *self = Motion { item: doing.item, light: self.light, lit: self.lit, ..Motion::default() };
        }
        self.raised += dt;
        // Strokes run their course, and a held button starts the next.
        let run = |stroke: &mut Option<f32>, start: bool, seconds: f32| {
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
        run(&mut self.attack, doing.swung, moves.attack.seconds);
        run(&mut self.dig, doing.digging && !doing.swung, moves.dig.seconds);
        run(&mut self.place, doing.using && !doing.guarding, moves.place.seconds);
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
        Weights {
            raise: smoothstep(self.raised / hand.raise_seconds),
            attack: weight(self.attack, moves.attack.seconds),
            dig: weight(self.dig, moves.dig.seconds),
            place: weight(self.place, moves.place.seconds),
            guard: smoothstep(self.guard),
            eat: smoothstep(self.eat),
            breath: self.breath * std::f32::consts::TAU,
            stride: self.stride * std::f32::consts::TAU,
            sway: (self.speed / hand.sway_speed).min(1.5),
        }
    }

    /// Whether a swing began: the rhythm's seconds into its swing dropped (a click starts
    /// the meter again from 0), not for a switch of what is held.
    pub fn swing_began(&mut self, since: f32, switched: bool) -> bool {
        let began = !switched && since < self.since_swing - 1e-3;
        self.since_swing = since;
        began
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
    add(moves.dig.at, moves.dig.turn, w.dig);
    add(moves.place.at, moves.place.turn, w.place);
    add(moves.guard.at, moves.guard.turn, w.guard);
    add(moves.eat.at, moves.eat.turn, w.eat);
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
) {
    let Some(handling) = &handled.0 else { return };
    let (dt, now) = (time.delta_secs(), time.elapsed_secs());
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
    let swung = motion.swing_began(since, switched) && !guarding && !eating;
    let locked = ui.cursor_locked && !player.dead;
    let doing = Doing {
        item,
        digging: locked && mouse.pressed(MouseButton::Left) && target.creature.is_none(),
        swung,
        using: locked && mouse.pressed(MouseButton::Right),
        guarding,
        eating,
        speed: player.velocity.xz().length(),
    };
    // The kind comes first, for the moves its strokes time.
    let Some(item) = item else {
        motion.step(doing, dt, &handling.hand, &handling.moves);
        return;
    };
    let eye_cell = player.eye().floor().as_ivec3();
    let look = look_of(&world, pieces.as_deref(), item, eye_cell);
    let def = world.registry.def(item);
    let kind = handling.kind_of(|tag| def.has_tag(tag), look.shape);
    let moves = handling.moves_of(kind);
    let weights = motion.step(doing, dt, &handling.hand, &moves);
    let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, &weights);
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
        for aspect in [4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            for kind in handling.kinds.iter().chain([&handling.cube, &handling.sprite]) {
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
                    ("dig", Weights { dig: 1.0, ..weights() }),
                    ("place", Weights { place: 1.0, ..weights() }),
                    ("guard", Weights { guard: 1.0, ..weights() }),
                    ("eat", Weights { eat: 1.0, ..weights() }),
                ] {
                    let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, &w);
                    let (middle, _) = face(at, turn, size, aspect);
                    assert!(at.z > 0.15, "{name}'s {move_name} goes behind the eye");
                    assert!(middle.x.abs() < 1.0 && middle.y.abs() < 1.0, "{name}'s {move_name} leaves the screen: its middle is at {middle}");
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
        let source = std::fs::read_to_string(assets().join("packs/default").join(FILE)).unwrap();
        let book = Book::parse(&source).unwrap();
        let changes: [(fn(&mut Book), &str); 6] = [
            (|b: &mut Book| b.kinds[0].pose.size = 0.0, "wide"),
            (|b: &mut Book| b.kinds[0].pose.at[2] = -0.5, "forward"),
            (|b: &mut Book| b.kinds[0].pose.at[0] = f32::NAN, "finite"),
            (|b: &mut Book| b.moves.attack.as_mut().unwrap().seconds = 0.0, "seconds"),
            (|b: &mut Book| b.hand.as_mut().unwrap().raise_seconds = 0.0, "raise_seconds"),
            (|b: &mut Book| b.kinds[1].tag = b.kinds[0].tag.clone(), "twice"),
        ];
        for (change, wanted) in changes {
            let mut broken = book.clone();
            change(&mut broken);
            let problems = broken.finish().unwrap_err();
            assert!(problems.iter().any(|p| p.contains(wanted)), "wanted `{wanted}` in {problems:?}");
        }
        assert!(Book::parse("(hand: 1)").is_err() && Book::parse("(nonsense: 1)").is_err());
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
        let f = 1.0 / (75f32.to_radians() / 2.0).tan();
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
            ("wooden_hoe", "hoe", vec![("rest", weights())]),
            ("fishing_rod", "rod", vec![("rest", weights())]),
            ("torch_lit", "light", vec![("rest", weights())]),
            ("cooked_rabbit", "edible", vec![("rest", weights()), ("eat", Weights { eat: 1.0, ..weights() })]),
        ];
        let panels: Vec<(String, Vec<u8>)> = sheet
            .iter()
            .flat_map(|(texture, tag, poses)| {
                let image = library.render(&format!("block/{texture}")).and_then(Result::ok);
                let kind = handling.kinds.iter().find(|k| k.tag == *tag).unwrap_or(&handling.sprite);
                let moves = handling.moves_of(kind);
                poses.iter().map(|(label, weights)| {
                    let (at, turn, size) = compose(&kind.pose, &moves, &handling.hand, weights);
                    let rotation = Quat::from_scaled_axis(turn_vector(turn));
                    let (centre, normal) = (Vec3::new(at.x, at.y, -at.z), rotation * Vec3::Z);
                    let mut pixels = vec![0u8; w * h * 3];
                    for y in 0..h {
                        for x in 0..w {
                            let (nx, ny) = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / h as f32 * 2.0);
                            // Sky above the horizon, grass below.
                            let mut rgb = if ny > -0.15 { [150u8, 195, 240] } else { [96, 150, 70] };
                            if x.abs_diff(w / 2) < 4 && y == h / 2 || y.abs_diff(h / 2) < 4 && x == w / 2 {
                                rgb = [255, 255, 255];
                            }
                            let direction = Vec3::new(nx * (w as f32 / h as f32) / f, ny / f, -1.0);
                            let t = centre.dot(normal) / direction.dot(normal);
                            if let (Some(image), true) = (&image, t > 0.0) {
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
}
