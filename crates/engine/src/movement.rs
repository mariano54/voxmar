//! How a player moves: walking, jumping, wading, swimming and flying, with collision
//! against the voxel grid. The client runs it to predict its own player; the server checks
//! the positions that come of it for plausibility. It knows the world only through
//! [`Surroundings`], so tests run it on any arrangement of blocks.
//!
//! Getting out of water follows Minecraft: water shallower than [`Tuning::wade_depth`] over
//! the feet is walked and jumped in, deeper water is swum in, and a swimmer pushing against a
//! bank whose top is within [`Tuning::step_out`] of their feet is lifted out at
//! [`Tuning::climb_out_speed`], so a bank level with the water is climbed by swimming up against it, without timing a jump.
//! A swimmer bobbing at the surface is in water too shallow to swim in, so holding jump
//! there jumps, as Minecraft's does, which takes them onto a bank a block above the water.
//!
//! Sprinting follows Minecraft too: the sprint key (or a double tap of forward, which the
//! client turns into [`Controls::sprint`]) starts it, and it lasts while forward is held,
//! until the player runs head-on into a wall.
//!
//! A ladder (a block tagged `climbable`) is climbed as in Minecraft: with the feet in its
//! cell, pressing against it (or against the wall behind) or holding jump goes up it at
//! [`Tuning::climb_speed`], and letting go slides down no faster than that.
//!
//! The speeds, gravity, jump and these depths and heights come from the content's
//! `movement.ron` ([`Tuning`]).

use glam::{IVec3, Vec2, Vec3};
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::fluid;
use crate::physics::{Aabb, CUBE, move_and_slide_boxes, move_axis_boxes, player as dims};

/// Longest step of the simulation, in seconds; longer frames are split, which keeps
/// collision stable at low frame rates.
pub const SUBSTEP: f32 = 1.0 / 120.0;

/// How players move, in blocks and seconds: the content's `movement.ron`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tuning {
    pub walk_speed: f32,
    pub sprint_speed: f32,
    /// Added along the way the player runs when they jump while sprinting.
    pub sprint_jump_boost: f32,
    pub jump_velocity: f32,
    pub gravity: f32,
    pub terminal_velocity: f32,
    /// Share of walking (or sprinting) speed kept in water.
    pub water_speed: f32,
    pub fly_speed: f32,
    pub fly_sprint_speed: f32,
    /// Water shallower than this over the feet is waded through: walked and jumped in,
    /// though slowly and carried by its flow, rather than swum in.
    pub wade_depth: f32,
    /// How far above the feet a bank may rise for a swimmer pushing against it to be
    /// lifted out, and how fast (blocks a second) they are lifted.
    pub step_out: f32,
    pub climb_out_speed: f32,
    /// Seconds after a jump before holding jump jumps again, so a jump from water doesn't
    /// repeat while the feet are still in it (at most 1).
    pub jump_delay: f32,
    /// How much of a wall-blocked step must have been heading into the wall for it to stop
    /// a sprint (a share, at most 1); glancing off a wall keeps it.
    pub head_on: f32,
    /// How high a walker on the ground steps up onto a block without jumping (at most 1):
    /// onto a slab, up a stair, over a low roof's courses, but not a whole block. The client
    /// steps by it and the server's move checks allow for it.
    pub step_up: f32,
    /// How fast a player climbs a ladder, and the fastest they slide down one, in blocks a
    /// second.
    pub climb_speed: f32,
    /// How much the server lets a player's moves get away with.
    pub checks: Checks,
}

/// How far the server lets the moves a client reports stray from these numbers, for
/// rounding, network jitter and hitches, before it puts the player back.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checks {
    /// How much faster than the top speeds a move may be.
    pub speed_margin: f32,
    /// Seconds of top speed a player's movement may run ahead in one go, and blocks of
    /// movement allowed besides.
    pub burst: f32,
    pub slack: f32,
    /// How far a player on foot may rise above where they last stood, besides their jump
    /// (and out of water, `Tuning::step_out`).
    pub rise_slack: f32,
    /// Seconds a player on foot may be off the ground before the height they may keep
    /// starts to sink, and how fast it sinks (blocks a second).
    pub hover_grace: f32,
    pub hover_sink: f32,
    /// Seconds a player let down to that height keeps it before it sinks again.
    pub let_down_pause: f32,
    /// How far above ground a player still counts as having just touched it.
    pub near_ground: f32,
    /// Longest step, in blocks, a move is checked for walls in.
    pub sweep_step: f32,
    /// The player's box, as shares of their width and height, that must stay out of
    /// walls: a little smaller than theirs, for rounding, raised `wall_margin` off their
    /// feet and inside their box less `wall_margin` all round. Only among whole blocks: a
    /// share would let a player through a gap beside a shape narrower than they are.
    pub wall_box: (f32, f32),
    /// Blocks the player's box is shrunk by on every side, for rounding, wherever a shaped
    /// block is near (`wall_box` among whole blocks only).
    pub wall_margin: f32,
}

crate::content::settable! {
    /// What one package's `movement.ron` sets of the `checks`.
    SetChecks for Checks {
        speed_margin: f32, burst: f32, slack: f32, rise_slack: f32, hover_grace: f32, hover_sink: f32,
        let_down_pause: f32, near_ground: f32, sweep_step: f32, wall_box: (f32, f32), wall_margin: f32,
    }
}

/// What a package's `movement.ron` sets after the first: any of the numbers, which replace
/// the earlier packages' (Design draft 91), and any of the `checks`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SetTuning {
    pub walk_speed: Option<f32>,
    pub sprint_speed: Option<f32>,
    pub sprint_jump_boost: Option<f32>,
    pub jump_velocity: Option<f32>,
    pub gravity: Option<f32>,
    pub terminal_velocity: Option<f32>,
    pub water_speed: Option<f32>,
    pub fly_speed: Option<f32>,
    pub fly_sprint_speed: Option<f32>,
    pub wade_depth: Option<f32>,
    pub step_out: Option<f32>,
    pub climb_out_speed: Option<f32>,
    pub jump_delay: Option<f32>,
    pub head_on: Option<f32>,
    pub step_up: Option<f32>,
    pub climb_speed: Option<f32>,
    pub checks: Option<SetChecks>,
}

impl crate::content::Settable for Tuning {
    type Set = SetTuning;

    fn with(mut self, set: &SetTuning) -> (Self, Vec<&'static str>) {
        let mut changed = Vec::new();
        macro_rules! lay {
            ($($field:ident),*) => {$(
                if let Some(value) = set.$field {
                    self.$field = value;
                    changed.push(stringify!($field));
                }
            )*};
        }
        lay!(walk_speed, sprint_speed, sprint_jump_boost, jump_velocity, gravity, terminal_velocity, water_speed, fly_speed, fly_sprint_speed, wade_depth, step_out, climb_out_speed, jump_delay, head_on, step_up, climb_speed);
        if let Some(checks) = &set.checks {
            let (laid, names) = self.checks.with(checks);
            self.checks = laid;
            changed.extend(names.into_iter().map(|name| match name {
                "speed_margin" => "checks.speed_margin",
                "burst" => "checks.burst",
                "slack" => "checks.slack",
                "rise_slack" => "checks.rise_slack",
                "hover_grace" => "checks.hover_grace",
                "hover_sink" => "checks.hover_sink",
                "let_down_pause" => "checks.let_down_pause",
                "near_ground" => "checks.near_ground",
                "sweep_step" => "checks.sweep_step",
                "wall_margin" => "checks.wall_margin",
                _ => "checks.wall_box",
            }));
        }
        (self, changed)
    }
}

impl Checks {
    fn check(&self) -> Result<(), String> {
        let Checks { speed_margin, burst, slack, rise_slack, hover_grace, hover_sink, let_down_pause, near_ground, sweep_step, wall_box: (width, height), wall_margin } = *self;
        let numbers = [speed_margin, burst, slack, rise_slack, hover_grace, hover_sink, let_down_pause, near_ground, sweep_step, width, height, wall_margin];
        if numbers.iter().any(|n| !n.is_finite() || *n < 0.0) {
            Err("every number must be zero or more".into())
        } else if speed_margin < 1.0 {
            Err("speed_margin below 1 would stop players at their own top speed".into())
        } else if burst <= 0.0 {
            Err("burst must be more than 0, or no move could be made".into())
        } else if !(0.05..=1.0).contains(&sweep_step) {
            Err("sweep_step must be from 0.05 to 1 (a smaller step makes every move slow to check, a larger one lets moves pass through walls)".into())
        } else if let_down_pause > hover_grace {
            Err("let_down_pause must be no more than hover_grace".into())
        } else if !(0.5..=1.0).contains(&width) || !(0.5..=1.0).contains(&height) {
            Err("wall_box shares must be from 0.5 to 1".into())
        } else if wall_margin > 0.015 {
            Err("wall_margin must be at most 0.015 (more lets a player through a gap of 9/16 of a block, narrower than they are)".into())
        } else if width * dims::WIDTH > dims::WIDTH - 2.0 * wall_margin || height * dims::HEIGHT > dims::HEIGHT - 2.0 * wall_margin {
            // The server judges a spot by one box or the other as shapes come and go beside
            // it; one inside the other, a spot clear by the margin is clear by the share too.
            Err("wall_box must fit inside the player's box less wall_margin on every side".into())
        } else {
            Ok(())
        }
    }
}

impl Tuning {
    pub fn from_ron(source: &str) -> Result<Self, RegistryError> {
        let tuning: Tuning = ron::from_str(source).map_err(|e| RegistryError(format!("movement.ron: {e}")))?;
        tuning.validate()?;
        Ok(tuning)
    }

    /// What a package after the first writes in its `movement.ron`: only the numbers it changes.
    pub fn set_from_ron(source: &str) -> Result<SetTuning, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(source).map_err(|e| RegistryError(format!("movement.ron: {e}")))
    }

    /// Refuses numbers that could hang or stop the server or the players, as a file or the
    /// packages' fields laid together set them.
    pub fn validate(&self) -> Result<(), RegistryError> {
        self.check().map_err(|e| RegistryError(format!("movement.ron: {e}")))?;
        self.checks.check().map_err(|e| RegistryError(format!("movement.ron: checks: {e}")))
    }

    fn check(&self) -> Result<(), String> {
        let Tuning { wade_depth, step_out, climb_out_speed, jump_delay, head_on, step_up, climb_speed, .. } = *self;
        let named = [("wade_depth", wade_depth), ("step_out", step_out), ("climb_out_speed", climb_out_speed), ("jump_delay", jump_delay), ("head_on", head_on), ("step_up", step_up), ("climb_speed", climb_speed)];
        if let Some((name, _)) = named.iter().find(|(_, n)| !n.is_finite() || *n <= 0.0) {
            Err(format!("{name} must be more than 0"))
        } else if jump_delay > 1.0 {
            Err("jump_delay must be at most 1 second".into())
        } else if head_on > 1.0 {
            Err("head_on must be at most 1 (a share of the way the player runs), or a sprint would never stop at a wall".into())
        } else if step_up > 1.0 {
            Err("step_up must be at most 1 block".into())
        } else if climb_speed > self.jump_velocity {
            // The server lets a player on a ladder rise as fast as a jump, no faster.
            Err("climb_speed must be at most jump_velocity".into())
        } else {
            Ok(())
        }
    }

    /// The tuning for a player whose effects make them `factor` times as quick on foot
    /// (flying is untouched).
    pub fn on_foot(&self, factor: f32) -> Tuning {
        Tuning {
            walk_speed: self.walk_speed * factor,
            sprint_speed: self.sprint_speed * factor,
            sprint_jump_boost: self.sprint_jump_boost * factor,
            ..self.clone()
        }
    }

    /// The fastest a player moves across the ground, sprint-jumping.
    pub fn top_ground_speed(&self) -> f32 {
        self.sprint_speed + self.sprint_jump_boost
    }
}

/// The moving state of a player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Feet position.
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub in_water: bool,
    pub flying: bool,
    /// Flight speed multiplier.
    pub fly_speed: f32,
    /// Whether the last step was stopped by a wall across, x or z.
    pub against_wall: bool,
    pub sprinting: bool,
    /// Seconds until holding jump may jump again (see [`Tuning::jump_delay`]).
    pub jump_delay: f32,
}

impl Default for Body {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            on_ground: false,
            in_water: false,
            flying: false,
            fly_speed: 1.0,
            against_wall: false,
            sprinting: false,
            jump_delay: 0.0,
        }
    }
}

impl Body {
    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, dims::WIDTH, dims::HEIGHT)
    }
}

/// What the player asks for this step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    /// Horizontal direction to move in, unit length or zero.
    pub wish: Vec3,
    /// Whether the forward key is held: sprinting needs it.
    pub forward: bool,
    /// Start sprinting: the sprint key is held, or forward was just tapped twice.
    pub sprint: bool,
    /// Jump, swim up, or fly up.
    pub jump: bool,
    /// Fly down.
    pub descend: bool,
}

/// What a moving body needs to know about the world around it.
pub trait Surroundings {
    /// Whether a cell stops movement.
    fn solid(&self, cell: IVec3) -> bool;
    /// What a cell stops movement with, in blocks from its lower corner (see
    /// [`BlockRegistry::collision`]): by default the whole cell if it is solid.
    fn boxes(&self, cell: IVec3) -> &[Aabb] {
        if self.solid(cell) { &CUBE } else { &[] }
    }
    /// How deep the water around a box is over its feet (0 out of water), and its mean
    /// flow (blocks per second along x, z).
    fn water(&self, aabb: &Aabb) -> (f32, Vec2);
    /// Whether a cell holds a ladder (see [`crate::block::BlockInfo::climbable`]): by
    /// default none does.
    fn climbable(&self, _cell: IVec3) -> bool {
        false
    }
}

/// Whether a body's feet are in a ladder's cell: the cells its footprint covers at the
/// height of its feet, so it climbs off the top of a ladder rather than a body's height
/// past it.
pub fn on_ladder(body: &Body, world: &impl Surroundings) -> bool {
    let aabb = body.aabb();
    let (lo, hi) = ((aabb.min + 1e-4).floor().as_ivec3(), (aabb.max - 1e-4).floor().as_ivec3());
    let y = body.position.y.floor() as i32;
    (lo.x..=hi.x).any(|x| (lo.z..=hi.z).any(|z| world.climbable(IVec3::new(x, y, z))))
}

/// Advances `body` by `dt` seconds (split into substeps of at most [`SUBSTEP`]).
pub fn advance(body: &mut Body, world: &impl Surroundings, tuning: &Tuning, controls: Controls, dt: f32) {
    let steps = (dt / SUBSTEP).ceil() as usize;
    if steps == 0 {
        return;
    }
    // Equal steps preserve the whole frame without a tiny subtraction remainder whose
    // downward displacement rounds away and clears on_ground after landing.
    let dt = dt / steps as f32;
    for _ in 0..steps {
        step(body, world, tuning, controls, dt);
    }
}

/// One substep of movement.
pub fn step(body: &mut Body, world: &impl Surroundings, tuning: &Tuning, controls: Controls, dt: f32) {
    let (depth, current) = world.water(&body.aabb());
    body.in_water = depth > 0.0;
    // By depth alone, as in Minecraft: a jump from shallow water is a jump, which judging
    // by the ground as well cut short to a swim stroke as soon as the feet left it.
    let swimming = depth >= tuning.wade_depth;
    let Controls { wish, forward, sprint, jump, descend } = controls;
    if !forward || wish == Vec3::ZERO {
        body.sprinting = false;
    } else if sprint {
        body.sprinting = true;
    }
    // Minecraft lets a held jump repeat only after a delay, which letting go clears.
    body.jump_delay = if jump { (body.jump_delay - dt).max(0.0) } else { 0.0 };

    if body.flying {
        let vertical = (jump as i32 - descend as i32) as f32;
        let speed = if body.sprinting { tuning.fly_sprint_speed } else { tuning.fly_speed } * body.fly_speed;
        let target = (wish + Vec3::Y * vertical) * speed;
        body.velocity = body.velocity.lerp(target, (12.0 * dt).min(1.0));
    } else {
        let speed = if body.sprinting { tuning.sprint_speed } else { tuning.walk_speed };
        let speed = speed * if body.in_water { tuning.water_speed } else { 1.0 };
        let control = if body.on_ground { 16.0 } else { 3.0 };
        // In water you move relative to the water, so a current carries you along.
        let target = wish * speed + Vec3::new(current.x, 0.0, current.y);
        let horizontal = body.velocity.with_y(0.0).lerp(target, (control * dt).min(1.0));
        body.velocity = horizontal.with_y(body.velocity.y);

        if swimming {
            body.velocity.y -= tuning.gravity * 0.15 * dt;
            body.velocity.y *= 1.0 - (2.5 * dt).min(1.0);
            if jump {
                body.velocity.y = (body.velocity.y + 25.0 * dt).min(3.5);
            }
        } else {
            // Off the ground, or from water too shallow to swim in even with nothing
            // underfoot: a swimmer bobbing up to the surface hops out of it, as in
            // Minecraft, high enough to reach a bank a block above the water.
            if jump && (body.on_ground || body.in_water) && body.jump_delay == 0.0 {
                body.velocity.y = body.velocity.y.max(tuning.jump_velocity);
                body.jump_delay = tuning.jump_delay;
                if body.sprinting && body.on_ground && !body.in_water {
                    body.velocity += wish * tuning.sprint_jump_boost;
                }
            }
            body.velocity.y = (body.velocity.y - tuning.gravity * dt).max(-tuning.terminal_velocity);
        }
        // On a ladder: pressing against it or holding jump climbs; otherwise a slow slide,
        // and nobody goes up one faster than they climb it (the server's checks hold them
        // to that).
        if !swimming && on_ladder(body, world) {
            body.velocity.y = if jump || (body.against_wall && wish != Vec3::ZERO) { tuning.climb_speed } else { body.velocity.y.clamp(-tuning.climb_speed, tuning.climb_speed) };
        }
        // Pushing against a bank low enough to step onto lifts a swimmer out.
        if body.in_water && body.against_wall && wish != Vec3::ZERO && can_step_out(body, world, wish, tuning.step_out) {
            body.velocity.y = body.velocity.y.max(tuning.climb_out_speed);
        }
    }

    let start = body.aabb();
    let motion = body.velocity * dt;
    let mut aabb = start;
    let mut blocked = move_and_slide_boxes(&mut aabb, motion, |p| world.boxes(p));
    // Stopped across while on the ground: stepping up onto what stopped it (a slab, a stair)
    // goes further, as in Minecraft, if it fits; whole blocks are still jumped onto.
    if (blocked[0] || blocked[2])
        && body.on_ground
        && !body.flying
        && !swimming
        && motion.y <= 0.0
        && tuning.step_up > 0.0
        && let Some((stepped, across)) = step_up(start, motion, tuning.step_up, world)
    {
        // Up and further across; or not yet over the step but no longer stopped by it,
        // so the walker keeps its speed and goes up next time: one starting off against
        // the step moves less in a short frame, far from the origin less than positions
        // round to, than it takes to come down on it.
        let gone = |b: &Aabb| (b.min - start.min).with_y(0.0).length_squared();
        let freed = [0, 2].into_iter().any(|k| blocked[k] && !across[k]);
        if freed || (stepped.min.y > start.min.y && gone(&stepped) > gone(&aabb) + 1e-8) {
            aabb = stepped;
            blocked = [across[0], true, across[2]];
        }
    }
    body.on_ground = blocked[1] && body.velocity.y <= 0.0;
    body.against_wall = blocked[0] || blocked[2];
    // Running head-on into a wall stops a sprint on the ground; glancing along one, or
    // bumping a step mid-jump, doesn't.
    let head_on = (blocked[0] && wish.x.abs() > tuning.head_on) || (blocked[2] && wish.z.abs() > tuning.head_on);
    if head_on && body.on_ground && !body.flying {
        body.sprinting = false;
    }
    for (axis, blocked) in blocked.into_iter().enumerate() {
        if blocked {
            body.velocity[axis] = 0.0;
        }
    }
    if body.on_ground && body.flying && !jump {
        body.flying = false;
    }
    body.position = Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0);
}

/// Where a box at `start` moving by `motion` ends if it first rises by up to `step_up`, then
/// goes across, then comes down onto what it stepped onto (and by `motion`'s own fall), with
/// which axes across stopped it; none if it would come down on nothing.
fn step_up(start: Aabb, motion: Vec3, step_up: f32, world: &impl Surroundings) -> Option<(Aabb, [bool; 3])> {
    let boxes = |p: IVec3| world.boxes(p);
    let mut aabb = start;
    move_axis_boxes(&mut aabb, 1, step_up, &boxes);
    let risen = aabb.min.y - start.min.y;
    let across = [move_axis_boxes(&mut aabb, 0, motion.x, &boxes), false, move_axis_boxes(&mut aabb, 2, motion.z, &boxes)];
    let landed = move_axis_boxes(&mut aabb, 1, -(risen - motion.y), &boxes);
    landed.then_some((aabb, across))
}

/// Whether the body, raised by `step_out` (`Tuning::step_out`), would have room a little
/// further along `wish`: the wall it is pushing against ends within a step of its feet.
fn can_step_out(body: &Body, world: &impl Surroundings, wish: Vec3, step_out: f32) -> bool {
    let mut raised = body.aabb();
    raised.translate(Vec3::Y * step_out + wish * 0.1);
    !raised.overlaps(|p| world.boxes(p))
}

/// How deep the water `aabb` stands in is over its feet (0 out of water), and the mean
/// flow of that water (blocks per second along x, z): flowing water and river currents
/// carry swimmers along. Only fluids that are swum in count (`FluidDef::swim`), so not
/// lava. The client moves by it and the server judges moves by it.
pub fn water_around(registry: &BlockRegistry, block: impl Fn(IVec3) -> Option<BlockId>, aabb: &Aabb) -> (f32, Vec2) {
    let (lo, hi) = (aabb.min.floor().as_ivec3(), (aabb.max - 1e-4).floor().as_ivec3());
    let (mut cells, mut depth, mut flow) = (0, 0.0f32, Vec2::ZERO);
    for y in lo.y..=hi.y {
        for z in lo.z..=hi.z {
            for x in lo.x..=hi.x {
                let cell = IVec3::new(x, y, z);
                let Some(state) = block(cell).and_then(|b| registry.fluid(b)).filter(|s| registry.fluid_def(s.fluid).def.swim) else { continue };
                // Shallow flowing water only counts up to its surface.
                let covered = block(cell + IVec3::Y).and_then(|b| registry.fluid(b)).is_some_and(|s| s.fluid == state.fluid);
                let surface = y as f32 + if covered { 1.0 } else { state.height() };
                if aabb.min.y < surface {
                    cells += 1;
                    depth = depth.max(surface - aabb.min.y);
                    flow += fluid::flow(registry, &block, cell);
                }
            }
        }
    }
    (depth, if cells > 0 { flow / cells as f32 } else { Vec2::ZERO })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solid cells, and water as a surface height over the ground.
    struct Scene<F: Fn(IVec3) -> bool> {
        solid: F,
        /// Water fills everything below this height.
        water: Option<f32>,
    }

    impl<F: Fn(IVec3) -> bool> Surroundings for Scene<F> {
        fn solid(&self, cell: IVec3) -> bool {
            (self.solid)(cell)
        }

        fn water(&self, aabb: &Aabb) -> (f32, Vec2) {
            (self.water.map_or(0.0, |surface| (surface - aabb.min.y).max(0.0)), Vec2::ZERO)
        }
    }

    fn tuning() -> Tuning {
        Tuning::from_ron(include_str!("../../../assets/content/core/movement.ron")).unwrap()
    }

    #[test]
    fn the_shipped_movement_file_moves_players_as_before_and_refuses_what_could_not_move_them() {
        // The numbers that were constants here, unchanged by moving them to data.
        let t = tuning();
        assert_eq!([t.wade_depth, t.step_out, t.climb_out_speed, t.jump_delay, t.head_on, t.step_up], [0.4, 0.6, 6.0, 0.5, 0.35, 0.5 + 1.0 / 16.0]);
        let source = include_str!("../../../assets/content/core/movement.ron");
        for (from, to) in [("wade_depth: 0.4", "wade_depth: 0.0"), ("climb_out_speed: 6.0", "climb_out_speed: -6.0"), ("jump_delay: 0.5", "jump_delay: 1.5"), ("head_on: 0.35", "head_on: 0.0"), ("head_on: 0.35", "head_on: 1.5"), ("step_up: 0.5625", "step_up: 2.0"), ("climb_speed: 3.0", "climb_speed: 0.0"), ("climb_speed: 3.0", "climb_speed: 10.0")] {
            assert!(source.contains(from), "{from}");
            assert!(Tuning::from_ron(&source.replace(from, to)).is_err(), "{to} loads");
        }
    }

    #[test]
    fn checks_a_mod_could_set_to_hang_or_stop_the_server_are_refused_on_load() {
        let source = include_str!("../../../assets/content/core/movement.ron");
        for (from, to) in [("sweep_step: 0.25", "sweep_step: 0.0"), ("speed_margin: 1.25", "speed_margin: 0.5"), ("burst: 2.0", "burst: 0.0"), ("let_down_pause: 0.25", "let_down_pause: 3.0"), ("hover_sink: 1.0", "hover_sink: -1.0"), ("wall_box: (0.9, 0.95)", "wall_box: (0.9, 2.0)"), ("wall_margin: 0.01", "wall_margin: 0.1"), ("wall_margin: 0.01", "wall_margin: 0.02"), ("wall_box: (0.9, 0.95)", "wall_box: (1.0, 0.95)"), ("wall_box: (0.9, 0.95)", "wall_box: (0.9, 0.995)")] {
            assert!(source.contains(from), "{from}");
            assert!(Tuning::from_ron(&source.replace(from, to)).is_err(), "{to} loads");
        }
    }

    #[test]
    fn a_later_file_sets_only_the_numbers_it_names_and_the_laid_result_is_checked() {
        use crate::content::Settable;
        let base = tuning();
        let set = Tuning::set_from_ron("(gravity: 20.0, checks: (burst: 3.0, wall_box: (0.8, 0.9)))").unwrap();
        let (laid, changed) = base.clone().with(&set);
        assert_eq!(changed, ["gravity", "checks.burst", "checks.wall_box"]);
        assert_eq!((laid.gravity, laid.checks.burst, laid.checks.wall_box), (20.0, 3.0, (0.8, 0.9)));
        assert_eq!((laid.walk_speed, laid.checks.slack), (base.walk_speed, base.checks.slack));
        laid.validate().unwrap();
        assert!(Tuning::set_from_ron("(gravty: 20.0)").is_err(), "a misspelt number is refused");
        // Each file may pass alone while what they lay to is out of range.
        let (broken, _) = base.clone().with(&Tuning::set_from_ron("(checks: (let_down_pause: 5.0))").unwrap());
        assert!(broken.validate().unwrap_err().0.contains("let_down_pause"));
    }

    /// Holds `controls` for `seconds` at `fps` frames a second; returns the body and
    /// whether it was ever inside a solid block.
    fn run(world: &impl Surroundings, mut body: Body, controls: Controls, seconds: f32, fps: f32) -> (Body, bool) {
        let (tuning, mut inside) = (tuning(), false);
        for _ in 0..(seconds * fps) as usize {
            advance(&mut body, world, &tuning, controls, 1.0 / fps);
            let mut core = body.aabb();
            core.min += 0.01;
            core.max -= 0.01;
            inside |= core.overlaps(|p| world.boxes(p));
        }
        (body, inside)
    }

    fn at(x: f32, y: f32, z: f32) -> Body {
        Body { position: Vec3::new(x, y, z), ..Body::default() }
    }

    const EAST: Controls = Controls { wish: Vec3::X, forward: true, sprint: false, jump: false, descend: false };
    const EAST_SPRINTING: Controls = Controls { sprint: true, ..EAST };
    const EAST_JUMPING: Controls = Controls { jump: true, ..EAST };

    #[test]
    fn landing_stays_grounded_across_slow_fast_and_mixed_frames() {
        let tuning = tuning();
        for x in [0.5, 20_000.5, -5_000.5] {
            for intervals in [vec![0.25], vec![1.0 / 12.0], vec![1.0 / 60.0], vec![1.0 / 500.0], vec![0.25, 1.0 / 60.0, 0.12, 1.0 / 12.0]] {
                let scene = Scene { solid: |c: IVec3| c.y < 201, water: None };
                let mut body = at(x, 202.0, 0.5);
                let (mut landed, mut elapsed, mut frame) = (false, 0.0, 0);
                while elapsed < 2.0 {
                    let dt = intervals[frame % intervals.len()];
                    advance(&mut body, &scene, &tuning, Controls::default(), dt);
                    if landed {
                        assert!(body.on_ground, "lost grounding at {x}, intervals {intervals:?}, frame {frame}: {body:?}");
                        assert_eq!(body.position.y, 201.0);
                        assert_eq!(body.velocity.y, 0.0);
                    }
                    landed |= body.on_ground;
                    elapsed += dt;
                    frame += 1;
                }
                assert!(landed, "never landed at {x}, intervals {intervals:?}");
                let before = body;
                advance(&mut body, &scene, &tuning, Controls::default(), 0.0);
                assert_eq!(body, before, "a zero frame does not change movement");
            }
        }
    }

    #[test]
    fn walking_into_a_wall_stops_at_it() {
        let scene = Scene { solid: |c: IVec3| c.y < 0 || (c.y <= 1 && c.x >= 2), water: None };
        let (body, inside) = run(&scene, at(0.5, 0.0, 0.5), EAST, 2.0, 60.0);
        assert!(!inside);
        assert!((body.position.x - 1.7).abs() < 1e-3);
    }

    /// Blocks by the boxes they stop movement with: a slab's lower half, a stair facing
    /// west (climbing east), whole cubes.
    struct Shaped<F: Fn(IVec3) -> &'static [Aabb]>(F);

    impl<F: Fn(IVec3) -> &'static [Aabb]> Surroundings for Shaped<F> {
        fn solid(&self, cell: IVec3) -> bool {
            !(self.0)(cell).is_empty()
        }

        fn boxes(&self, cell: IVec3) -> &[Aabb] {
            (self.0)(cell)
        }

        fn water(&self, _: &Aabb) -> (f32, Vec2) {
            (0.0, Vec2::ZERO)
        }
    }

    const SLAB: [Aabb; 1] = [Aabb { min: Vec3::ZERO, max: Vec3::new(1.0, 0.5, 1.0) }];
    const STAIR: [Aabb; 2] = [Aabb { min: Vec3::ZERO, max: Vec3::new(1.0, 0.5, 1.0) }, Aabb { min: Vec3::new(0.5, 0.5, 0.0), max: Vec3::ONE }];

    /// Walking east without jumping goes up a slab, a stair and onto a whole block beyond,
    /// at slow and fast frame rates, near the origin and far out; never inside anything.
    #[test]
    fn walking_steps_up_slabs_and_stairs_without_jumping() {
        for base in [0.0f32, 2_100.0, -5_000.0] {
            let b = base.floor() as i32;
            let scene = Shaped(move |c: IVec3| -> &'static [Aabb] {
                match (c.x - b, c.y) {
                    (_, y) if y < 0 => &CUBE,
                    (2, 0) => &SLAB,
                    (3, 0) => &STAIR,
                    (4.., 0) => &CUBE,
                    (6.., 1) => &SLAB,
                    _ => &[],
                }
            });
            for fps in [30.0, 60.0, 144.0, 500.0] {
                let (body, inside) = run(&scene, at(base + 0.5, 0.0, 0.5), EAST, 3.0, fps);
                assert!(!inside, "went into a block at {base}, {fps} fps: {body:?}");
                assert!(body.on_ground && (body.position.y - 1.5).abs() < 1e-4, "did not climb to the slab on the block at {base}, {fps} fps: {body:?}");
                assert!(body.position.x - base > 6.0, "stopped at {} at {base}, {fps} fps", body.position.x - base);
            }
        }
    }

    /// The game's own blocks set about the origin, by the boxes they stop movement with;
    /// stone under y 0 unless `floor` says how deep the ground lies at a cell's x and z.
    struct Built<'a> {
        blocks: &'a crate::block::BlockRegistry,
        cells: rustc_hash::FxHashMap<IVec3, crate::block::BlockId>,
        floor: fn(IVec3) -> i32,
    }

    impl Surroundings for Built<'_> {
        fn solid(&self, cell: IVec3) -> bool {
            !self.boxes(cell).is_empty()
        }

        fn boxes(&self, cell: IVec3) -> &[Aabb] {
            match self.cells.get(&cell) {
                Some(&id) => self.blocks.collision(id),
                None if cell.y < (self.floor)(cell) => &CUBE,
                None => &[],
            }
        }

        fn water(&self, _: &Aabb) -> (f32, Vec2) {
            (0.0, Vec2::ZERO)
        }

        fn climbable(&self, cell: IVec3) -> bool {
            self.cells.get(&cell).is_some_and(|&id| self.blocks.info(id).climbable)
        }
    }

    fn content() -> crate::content::Content {
        crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads")
    }

    fn walking(wish: Vec3, sprint: bool) -> Controls {
        Controls { wish: wish.normalize(), forward: true, sprint, jump: false, descend: false }
    }

    /// Every stair in the game, wood or stone, in a flight of three up to a landing walled at
    /// its far end: walked
    /// south without jumping, the player climbs it step by step onto the landing, level
    /// with the top stair, and walked back north comes down it to the ground, never inside
    /// a block, at a slow and a fast frame rate.
    #[test]
    fn every_stair_is_walked_up_to_a_landing_and_down_again() {
        let content = content();
        let blocks = &content.blocks;
        let (stone, landing) = (blocks.expect("base:stone"), blocks.expect("base:oak_landing"));
        let stairs: Vec<_> = blocks.tagged("stairs").filter(|&id| !["_east", "_south", "_west", "_top"].iter().any(|s| blocks.def(id).name.ends_with(s))).collect();
        assert!(stairs.len() >= 5, "the woods' stairs and the stone ones: {}", stairs.len());
        for stair in stairs {
            let name = &blocks.def(stair).name;
            let mut cells = rustc_hash::FxHashMap::default();
            for step in 0..3 {
                cells.insert(IVec3::new(0, step, 2 + step), stair);
                for y in 0..step {
                    cells.insert(IVec3::new(0, y, 2 + step), stone);
                }
            }
            for z in 5..=6 {
                cells.insert(IVec3::new(0, 2, z), landing);
                cells.insert(IVec3::new(0, 1, z), stone);
                cells.insert(IVec3::new(0, 0, z), stone);
            }
            // A wall at the landing's far end, so the walk up stops on it.
            for y in 0..5 {
                cells.insert(IVec3::new(0, y, 7), stone);
            }
            let flight = Built { blocks, cells, floor: |_| 0 };
            for fps in [30.0, 144.0] {
                let (up, inside) = run(&flight, at(0.5, 0.0, 0.5), walking(Vec3::Z, false), 4.0, fps);
                assert!(!inside, "{name}: went into a block going up at {fps} fps: {up:?}");
                assert!(up.on_ground && (up.position.y - 3.0).abs() < 1e-4 && up.position.z > 5.0, "{name}: did not climb onto the landing at {fps} fps: {up:?}");
                let (down, inside) = run(&flight, Body { velocity: Vec3::ZERO, ..up }, walking(Vec3::NEG_Z, false), 4.0, fps);
                assert!(!inside, "{name}: went into a block going down at {fps} fps: {down:?}");
                assert!(down.on_ground && down.position.y.abs() < 1e-4 && down.position.z < 1.5, "{name}: did not come down to the ground at {fps} fps: {down:?}");
            }
        }
    }

    /// Every roof corner, hip and valley, thatch and shingles, walked up course by course
    /// from where its eaves meet without jumping, along the diagonal (a hip's ridge, a
    /// valley's gutter), to its top a block up, never inside it.
    #[test]
    fn every_roof_corner_is_walked_up_course_by_course() {
        let content = content();
        let blocks = &content.blocks;
        let stone = blocks.expect("base:stone");
        let corners: Vec<_> = blocks.tagged("roof_corner").filter(|&id| !["_east", "_south", "_west"].iter().any(|s| blocks.def(id).name.ends_with(s))).collect();
        assert!(corners.len() >= 8, "a hip and a valley in thatch and three woods: {}", corners.len());
        for corner in corners {
            let name = &blocks.def(corner).name;
            let mut cells = rustc_hash::FxHashMap::default();
            cells.insert(IVec3::new(0, 0, 0), corner);
            // A wall behind its top course, so the walk stops on it.
            for (x, z) in [(1, 0), (0, 1), (1, 1)] {
                for y in 0..3 {
                    cells.insert(IVec3::new(x, y, z), stone);
                }
            }
            let roof = Built { blocks, cells, floor: |_| 0 };
            for fps in [30.0, 144.0] {
                let (body, inside) = run(&roof, Body { on_ground: true, ..at(-0.6, 0.0, -0.6) }, walking(Vec3::new(1.0, 0.0, 1.0), false), 3.0, fps);
                assert!(!inside, "{name}: went into it at {fps} fps: {body:?}");
                assert!(body.on_ground && (body.position.y - 1.0).abs() < 1e-4, "{name}: did not climb to its top at {fps} fps: {body:?}");
            }
        }
    }

    /// A balcony four blocks over the ground, railed along its east and south edges with a
    /// corner where they meet: walked or sprinted at the railing, straight or across the
    /// corner, the player stops at it, on the balcony, never inside it and never over.
    #[test]
    fn a_railing_keeps_a_walker_on_the_balcony() {
        let content = content();
        let blocks = &content.blocks;
        let mut cells = rustc_hash::FxHashMap::default();
        for i in -3..3 {
            cells.insert(IVec3::new(3, 0, i), blocks.expect("base:oak_railing_west"));
            cells.insert(IVec3::new(i, 0, 3), blocks.expect("base:oak_railing"));
        }
        cells.insert(IVec3::new(3, 0, 3), blocks.expect("base:oak_railing_corner"));
        // The balcony's floor reaches x and z 3; beyond them the ground is four down.
        let balcony = Built { blocks, cells, floor: |c| if c.x <= 3 && c.z <= 3 { 0 } else { -4 } };
        for wish in [Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)] {
            for sprint in [false, true] {
                for fps in [30.0, 144.0] {
                    let (body, inside) = run(&balcony, Body { on_ground: true, ..at(0.5, 0.0, 0.5) }, walking(wish, sprint), 3.0, fps);
                    let kept = body.position.x < 4.0 && body.position.z < 4.0 && body.position.y.abs() < 1e-4 && body.on_ground;
                    assert!(!inside && kept, "walking {wish} (sprint {sprint}) at {fps} fps went past the railing: {body:?}");
                }
            }
        }
    }

    /// A row of field fences, of fence corners or of shut gates across a field stops a
    /// walker at it, walked or sprinted, never inside and never over; a row of open gates
    /// lets it through.
    #[test]
    fn a_fence_or_a_shut_gate_stops_a_walker_and_an_open_gate_lets_it_through() {
        let content = content();
        let blocks = &content.blocks;
        for (name, passes) in [("base:oak_fence_east", false), ("base:oak_fence_corner", false), ("base:oak_fence_gate_east", false), ("base:oak_fence_gate_open_east", true)] {
            let mut cells = rustc_hash::FxHashMap::default();
            for z in -3..=3 {
                cells.insert(IVec3::new(3, 0, z), blocks.expect(name));
            }
            let field = Built { blocks, cells, floor: |_| 0 };
            for sprint in [false, true] {
                for fps in [30.0, 144.0] {
                    let (body, inside) = run(&field, Body { on_ground: true, ..at(0.5, 0.0, 0.5) }, walking(Vec3::X, sprint), 3.0, fps);
                    let past = body.position.x > 4.0;
                    assert!(!inside && past == passes, "walking east (sprint {sprint}) at {fps} fps into a row of {name}: past it {past}, inside it {inside}: {body:?}");
                }
            }
        }
    }

    /// A ladder against a wall three blocks high: walked into, the player climbs it and
    /// steps off its top onto the wall, at any frame rate, never inside either, as they do
    /// up a shaft a block across onto the floor round its top; let go of
    /// halfway up, they slide down no faster than `climb_speed` to the ground; and holding
    /// jump climbs a ladder standing on its own, with nothing to press against.
    #[test]
    fn a_ladder_is_climbed_onto_the_wall_behind_and_slid_down_when_let_go() {
        let content = content();
        let blocks = &content.blocks;
        let tuning = tuning();
        let mut cells = rustc_hash::FxHashMap::default();
        for y in 0..3 {
            cells.insert(IVec3::new(0, y, 0), blocks.expect("base:oak_ladder"));
            for x in -2..=2 {
                for z in 1..=6 {
                    cells.insert(IVec3::new(x, y, z), blocks.expect("base:oak_planks"));
                }
            }
        }
        let wall = Built { blocks, cells, floor: |_| 0 };
        for fps in [30.0, 60.0, 144.0] {
            let (body, inside) = run(&wall, Body { on_ground: true, ..at(0.5, 0.0, -1.5) }, walking(Vec3::Z, false), 3.0, fps);
            assert!(!inside && body.on_ground && body.position.y == 3.0 && body.position.z > 1.0, "did not climb the ladder onto the wall at {fps} fps: {body:?}");
        }
        let mut body = at(0.5, 2.0, 0.5);
        let mut fastest = 0.0f32;
        for _ in 0..240 {
            advance(&mut body, &wall, &tuning, Controls::default(), 1.0 / 120.0);
            fastest = fastest.max(-body.velocity.y);
        }
        assert!(body.on_ground && body.position.y == 0.0, "did not slide down the ladder to the ground: {body:?}");
        assert!(fastest <= tuning.climb_speed + 1e-4, "slid down a ladder at {fastest} blocks a second");
        let mut cells = rustc_hash::FxHashMap::default();
        for y in 0..3 {
            cells.insert(IVec3::new(0, y, 0), blocks.expect("base:oak_ladder"));
        }
        let alone = Built { blocks, cells, floor: |_| 0 };
        // Up a shaft a block across, walled about, out onto the floor at its top.
        let mut cells = rustc_hash::FxHashMap::default();
        for y in 0..4 {
            for x in -2..=2 {
                for z in -2..=12 {
                    let cell = IVec3::new(x, y, z);
                    if (x, z) != (0, 0) {
                        cells.insert(cell, blocks.expect("base:oak_planks"));
                    }
                }
            }
            cells.insert(IVec3::new(0, y, 0), blocks.expect("base:oak_ladder"));
        }
        let shaft = Built { blocks, cells, floor: |_| 0 };
        for fps in [30.0, 60.0, 144.0] {
            let (body, inside) = run(&shaft, Body { on_ground: true, ..at(0.5, 0.0, 0.5) }, walking(Vec3::Z, false), 3.0, fps);
            assert!(!inside && body.on_ground && body.position.y == 4.0 && body.position.z > 1.0, "did not climb out of the shaft at {fps} fps: {body:?}");
        }
        let (body, _) = run(&alone, Body { on_ground: true, ..at(0.5, 0.0, 0.5) }, Controls { jump: true, ..Controls::default() }, 0.5, 60.0);
        assert!(body.position.y > 1.0 && body.velocity.y == tuning.climb_speed, "holding jump did not climb a ladder: {body:?}");
        // Thrown up into a ladder faster than it is climbed (a jump from beside it), or
        // falling into one faster than it is slid down, the body goes at `climb_speed`.
        for thrown in [tuning.jump_velocity, -20.0] {
            let mut body = Body { velocity: Vec3::Y * thrown, ..at(0.5, 1.0, 0.5) };
            advance(&mut body, &alone, &tuning, Controls::default(), 1.0 / 120.0);
            assert!(body.velocity.y.abs() <= tuning.climb_speed, "into a ladder at {thrown}: {body:?}");
        }
    }

    /// A ladder up a wall to a loft floor three blocks up, through a hatch: open, the player
    /// climbs on up through it and steps off onto the loft floor; shut, they are stopped
    /// under it, never inside it. And a shut hatch is floor: walked onto from the loft
    /// floor round it, the player stays up.
    #[test]
    fn a_ladder_climbs_through_an_open_hatch_into_a_loft_and_a_shut_one_stops_it() {
        let content = content();
        let blocks = &content.blocks;
        for (hatch, through) in [("base:oak_hatch_open", true), ("base:oak_hatch", false)] {
            let mut cells = rustc_hash::FxHashMap::default();
            for x in -3..=3 {
                for z in -3..=12 {
                    cells.insert(IVec3::new(x, 3, z), blocks.expect("base:oak_planks"));
                    if z >= 1 {
                        for y in 0..3 {
                            cells.insert(IVec3::new(x, y, z), blocks.expect("base:oak_planks"));
                        }
                    }
                }
            }
            for y in 0..3 {
                cells.insert(IVec3::new(0, y, 0), blocks.expect("base:oak_ladder"));
            }
            cells.insert(IVec3::new(0, 3, 0), blocks.expect(hatch));
            let loft = Built { blocks, cells, floor: |_| 0 };
            for fps in [30.0, 60.0, 144.0] {
                let (body, inside) = run(&loft, Body { on_ground: true, ..at(0.5, 0.0, -1.5) }, walking(Vec3::Z, false), 3.0, fps);
                assert!(!inside, "{hatch} at {fps} fps: went into a block: {body:?}");
                if through {
                    assert!(body.on_ground && body.position.y == 4.0 && body.position.z > 1.0, "did not climb through the open hatch at {fps} fps: {body:?}");
                } else {
                    assert!(body.position.y < 2.0 && body.position.z < 1.0, "went past the shut hatch at {fps} fps: {body:?}");
                }
            }
            if !through {
                let (body, inside) = run(&loft, Body { on_ground: true, ..at(0.5, 4.0, -2.5) }, walking(Vec3::Z, false), 1.5, 60.0);
                assert!(!inside && body.on_ground && body.position.y == 4.0 && body.position.z > 1.0, "fell through a shut hatch: {body:?}");
            }
        }
    }

    /// A whole block is no step: walking into it without jumping stops at it, as does a slab
    /// under a ceiling too low to stand under on it.
    #[test]
    fn whole_blocks_and_low_ceilings_are_not_stepped_onto() {
        let block = Shaped(|c: IVec3| -> &'static [Aabb] { if c.y < 0 || (c.y == 0 && c.x >= 2) { &CUBE } else { &[] } });
        let (body, _) = run(&block, at(0.5, 0.0, 0.5), EAST, 2.0, 60.0);
        assert!((body.position.x - 1.7).abs() < 1e-3 && body.position.y == 0.0, "{body:?}");
        let low = Shaped(|c: IVec3| -> &'static [Aabb] {
            match (c.x, c.y) {
                (_, y) if y < 0 => &CUBE,
                (2.., 0) => &SLAB,
                (2.., 2) => &CUBE,
                _ => &[],
            }
        });
        let (body, inside) = run(&low, at(0.5, 0.0, 0.5), EAST, 2.0, 60.0);
        assert!(!inside && body.position.y == 0.0 && (body.position.x - 1.7).abs() < 1e-3, "{body:?}");
    }

    /// Stepping up is for walkers on the ground: one falling past a slab's side, or flying,
    /// is stopped by it.
    #[test]
    fn only_walkers_on_the_ground_step_up() {
        let scene = Shaped(|c: IVec3| -> &'static [Aabb] { if c.y < 0 { &CUBE } else if c.x >= 2 && c.y == 3 { &SLAB } else { &[] } });
        let tuning = tuning();
        let mut body = at(1.5, 3.2, 0.5);
        body.velocity = Vec3::new(4.0, -2.0, 0.0);
        step(&mut body, &scene, &tuning, EAST, 1.0 / 60.0);
        assert!(body.position.y < 3.2 && body.position.x <= 1.7 + 1e-4, "{body:?}");
    }

    #[test]
    fn jumping_against_a_block_climbs_onto_it_anywhere() {
        // A one-block step, from the origin to far out where positions round coarsely, at
        // slow and fast frame rates.
        for base in [0.0f32, 2_100.0, -5_000.0, 20_000.0] {
            for fps in [30.0, 60.0, 144.0, 500.0] {
                let step = base.floor() as i32 + 2;
                let scene = Scene { solid: move |c: IVec3| c.y < 0 || (c.y == 0 && c.x >= step), water: None };
                let (body, inside) = run(&scene, at(base + 1.5, 0.0, base + 0.5), EAST_JUMPING, 1.5, fps);
                assert!(!inside, "inside a block at {base}, {fps} fps");
                assert!(body.position.x > step as f32 + 1.0, "never got onto the step at {base}, {fps} fps: {}", body.position);
                assert!(body.position.y >= 1.0 - 1e-3, "below the step's top at {base}, {fps} fps");
            }
        }
    }

    #[test]
    fn swimmers_climb_out_onto_a_bank_level_with_the_water() {
        // Water two blocks deep (surface at 0) against a bank level with it, from x = 2.
        let scene = Scene { solid: |c: IVec3| c.y < -2 || (c.y < 0 && c.x >= 2), water: Some(0.0) };
        let (body, inside) = run(&scene, at(1.0, -1.6, 0.5), EAST_JUMPING, 2.0, 60.0);
        assert!(!inside);
        // On the bank, perhaps still hopping as jump is held.
        assert!(body.position.x > 2.5 && body.position.y >= -1e-3, "still in the water: {}", body.position);
        assert!(!body.in_water);
        // Without swimming up, pushing against the bank with the feet within a step of its
        // top does it too, as in Minecraft.
        let (body, _) = run(&scene, at(1.7, -0.5, 0.5), EAST, 2.0, 60.0);
        assert!(body.position.x > 2.5, "{}", body.position);
    }

    #[test]
    fn swimmers_jump_out_onto_a_bank_a_block_above_the_water() {
        // Deep water against a bank whose top is a block above the surface: swimming up
        // and holding jump hops out onto it, whatever the frame rate.
        for fps in [30.0, 60.0, 144.0] {
            let scene = Scene { solid: |c: IVec3| c.y < -2 || (c.y < 1 && c.x >= 2), water: Some(0.0) };
            let (body, inside) = run(&scene, at(1.0, -1.6, 0.5), EAST_JUMPING, 3.0, fps);
            assert!(!inside);
            assert!(body.position.x > 2.5 && body.position.y >= 1.0 - 1e-3, "still in the water at {fps} fps: {}", body.position);
        }
    }

    #[test]
    fn swimmers_cannot_climb_a_bank_two_blocks_above_the_water() {
        let scene = Scene { solid: |c: IVec3| c.y < -2 || (c.y < 2 && c.x >= 2), water: Some(0.0) };
        let (body, _) = run(&scene, at(1.0, -1.6, 0.5), EAST_JUMPING, 3.0, 60.0);
        assert!(body.position.x < 2.0, "climbed two blocks out of water");
    }

    #[test]
    fn waders_jump_out_onto_a_block() {
        // Water a third of a block deep, and a block-high bank ahead.
        let scene = Scene { solid: |c: IVec3| c.y < 0 || (c.y == 0 && c.x >= 2), water: Some(0.3) };
        let (body, inside) = run(&scene, Body { on_ground: true, ..at(1.0, 0.0, 0.5) }, EAST_JUMPING, 1.5, 60.0);
        assert!(!inside);
        assert!(body.position.x > 2.5 && body.position.y >= 1.0 - 1e-3, "{}", body.position);
    }

    fn flat(c: IVec3) -> bool {
        c.y < 0
    }

    #[test]
    fn sprinting_is_faster_and_lasts_while_forward_is_held() {
        let scene = Scene { solid: flat, water: None };
        let grounded = Body { on_ground: true, ..at(0.5, 0.0, 0.5) };
        let (walked, _) = run(&scene, grounded, EAST, 1.0, 60.0);
        let (sprinted, _) = run(&scene, grounded, EAST_SPRINTING, 1.0, 60.0);
        assert!(sprinted.sprinting);
        assert!(sprinted.position.x > walked.position.x * 1.2, "{} vs {}", sprinted.position.x, walked.position.x);
        // Letting go of the sprint key keeps the sprint; letting go of forward ends it.
        let (body, _) = run(&scene, sprinted, EAST, 0.5, 60.0);
        assert!(body.sprinting);
        let (body, _) = run(&scene, body, Controls { forward: false, ..EAST }, 0.1, 60.0);
        assert!(!body.sprinting);
    }

    #[test]
    fn running_head_on_into_a_wall_stops_a_sprint_but_glancing_does_not() {
        let scene = Scene { solid: |c: IVec3| c.y < 0 || (c.y <= 1 && c.x >= 3), water: None };
        let grounded = Body { on_ground: true, ..at(0.5, 0.0, 0.5) };
        let (body, _) = run(&scene, grounded, EAST_SPRINTING, 0.2, 60.0);
        assert!(body.sprinting);
        let (body, _) = run(&scene, body, EAST, 1.0, 60.0);
        assert!(!body.sprinting, "still sprinting into the wall");
        // Along the wall, nudged a little into it.
        let along = Controls { wish: Vec3::new(0.2, 0.0, 1.0).normalize(), ..EAST_SPRINTING };
        let (body, _) = run(&scene, Body { on_ground: true, ..at(2.6, 0.0, 0.5) }, along, 1.0, 60.0);
        assert!(body.sprinting, "a glancing touch stopped the sprint");
    }

    #[test]
    fn sprint_jumping_is_fastest() {
        let scene = Scene { solid: flat, water: None };
        let grounded = Body { on_ground: true, ..at(0.5, 0.0, 0.5) };
        let (sprinted, _) = run(&scene, grounded, EAST_SPRINTING, 3.0, 60.0);
        let (jumped, _) = run(&scene, grounded, Controls { jump: true, ..EAST_SPRINTING }, 3.0, 60.0);
        assert!(jumped.position.x > sprinted.position.x * 1.05, "{} vs {}", jumped.position.x, sprinted.position.x);
        // Within what the server allows.
        assert!(jumped.position.x - 0.5 < tuning().top_ground_speed() * 3.0);
    }
}
