//! How a player moves: walking, jumping, wading, swimming and flying, with collision
//! against the voxel grid. The client runs it to predict its own player; the server checks
//! the positions that come of it for plausibility. It knows the world only through
//! [`Surroundings`], so tests run it on any arrangement of blocks.
//!
//! Getting out of water follows Minecraft: water shallower than [`WADE_DEPTH`] over the feet
//! is walked and jumped in, deeper water is swum in, and a swimmer pushing against a bank
//! whose top is within [`STEP_OUT`] of their feet is lifted out at [`CLIMB_OUT_SPEED`], so a
//! bank level with the water is climbed by swimming up against it, without timing a jump.

use glam::{IVec3, Vec2, Vec3};

use crate::block::{BlockId, BlockRegistry};
use crate::fluid;
use crate::physics::{Aabb, move_and_slide, player as dims};

/// Water shallower than this over the feet is waded through: walked and jumped in, though
/// slowly and carried by its flow, rather than swum in.
pub const WADE_DEPTH: f32 = 0.4;
/// How far above the feet a bank may rise for a swimmer pushing against it to be lifted
/// out (Minecraft's 0.6).
pub const STEP_OUT: f32 = 0.6;
/// Blocks per second a swimmer is lifted out at (Minecraft's 0.3 a tick).
pub const CLIMB_OUT_SPEED: f32 = 6.0;
/// Longest step of the simulation, in seconds; longer frames are split, which keeps
/// collision stable at low frame rates.
pub const SUBSTEP: f32 = 1.0 / 120.0;

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
    /// How deep the water around a box is over its feet (0 out of water), and its mean
    /// flow (blocks per second along x, z).
    fn water(&self, aabb: &Aabb) -> (f32, Vec2);
}

/// Advances `body` by `dt` seconds (split into substeps of at most [`SUBSTEP`]).
pub fn advance(body: &mut Body, world: &impl Surroundings, controls: Controls, dt: f32) {
    let mut remaining = dt;
    while remaining > 0.0 {
        let dt = remaining.min(SUBSTEP);
        remaining -= dt;
        step(body, world, controls, dt);
    }
}

/// One substep of movement.
pub fn step(body: &mut Body, world: &impl Surroundings, controls: Controls, dt: f32) {
    let (depth, current) = world.water(&body.aabb());
    body.in_water = depth > 0.0;
    // By depth alone, as in Minecraft: a jump from shallow water is a jump, which judging
    // by the ground as well cut short to a swim stroke as soon as the feet left it.
    let swimming = depth >= WADE_DEPTH;
    let Controls { wish, sprint, jump, descend } = controls;

    if body.flying {
        let vertical = (jump as i32 - descend as i32) as f32;
        let speed = if sprint { dims::FLY_SPRINT_SPEED } else { dims::FLY_SPEED } * body.fly_speed;
        let target = (wish + Vec3::Y * vertical) * speed;
        body.velocity = body.velocity.lerp(target, (12.0 * dt).min(1.0));
    } else {
        let speed = if sprint { dims::SPRINT_SPEED } else { dims::WALK_SPEED } * if body.in_water { 0.5 } else { 1.0 };
        let control = if body.on_ground { 16.0 } else { 3.0 };
        // In water you move relative to the water, so a current carries you along.
        let target = wish * speed + Vec3::new(current.x, 0.0, current.y);
        let horizontal = body.velocity.with_y(0.0).lerp(target, (control * dt).min(1.0));
        body.velocity = horizontal.with_y(body.velocity.y);

        if swimming {
            body.velocity.y -= dims::GRAVITY * 0.15 * dt;
            body.velocity.y *= 1.0 - (2.5 * dt).min(1.0);
            if jump {
                body.velocity.y = (body.velocity.y + 25.0 * dt).min(3.5);
            }
        } else {
            if jump && body.on_ground {
                body.velocity.y = dims::JUMP_VELOCITY;
            }
            body.velocity.y = (body.velocity.y - dims::GRAVITY * dt).max(-dims::TERMINAL_VELOCITY);
        }
        // Pushing against a bank low enough to step onto lifts a swimmer out.
        if body.in_water && body.against_wall && wish != Vec3::ZERO && can_step_out(body, world, wish) {
            body.velocity.y = body.velocity.y.max(CLIMB_OUT_SPEED);
        }
    }

    let mut aabb = body.aabb();
    let blocked = move_and_slide(&mut aabb, body.velocity * dt, |p| world.solid(p));
    body.on_ground = blocked[1] && body.velocity.y <= 0.0;
    body.against_wall = blocked[0] || blocked[2];
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

/// Whether the body, raised by [`STEP_OUT`], would have room a little further along
/// `wish`: the wall it is pushing against ends within a step of its feet.
fn can_step_out(body: &Body, world: &impl Surroundings, wish: Vec3) -> bool {
    let mut raised = body.aabb();
    raised.translate(Vec3::Y * STEP_OUT + wish * 0.1);
    !raised.intersects(|p| world.solid(p))
}

/// How deep the water `aabb` stands in is over its feet (0 out of water), and the mean
/// flow of that water (blocks per second along x, z): flowing water and river currents
/// carry swimmers along. Only `water` counts, not other fluids.
pub fn water_around(registry: &BlockRegistry, water: BlockId, block: impl Fn(IVec3) -> Option<BlockId>, aabb: &Aabb) -> (f32, Vec2) {
    let Some(water) = registry.fluid(water).map(|f| f.fluid) else { return (0.0, Vec2::ZERO) };
    let (lo, hi) = (aabb.min.floor().as_ivec3(), (aabb.max - 1e-4).floor().as_ivec3());
    let (mut cells, mut depth, mut flow) = (0, 0.0f32, Vec2::ZERO);
    for y in lo.y..=hi.y {
        for z in lo.z..=hi.z {
            for x in lo.x..=hi.x {
                let cell = IVec3::new(x, y, z);
                let Some(state) = block(cell).and_then(|b| registry.fluid(b)).filter(|s| s.fluid == water) else { continue };
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

    /// Holds `controls` for `seconds` at `fps` frames a second; returns the body and
    /// whether it was ever inside a solid block.
    fn run(world: &impl Surroundings, mut body: Body, controls: Controls, seconds: f32, fps: f32) -> (Body, bool) {
        let mut inside = false;
        for _ in 0..(seconds * fps) as usize {
            advance(&mut body, world, controls, 1.0 / fps);
            let mut core = body.aabb();
            core.min += 0.01;
            core.max -= 0.01;
            inside |= core.intersects(|p| world.solid(p));
        }
        (body, inside)
    }

    fn at(x: f32, y: f32, z: f32) -> Body {
        Body { position: Vec3::new(x, y, z), ..Body::default() }
    }

    const EAST: Controls = Controls { wish: Vec3::X, sprint: false, jump: false, descend: false };
    const EAST_JUMPING: Controls = Controls { jump: true, ..EAST };

    #[test]
    fn walking_into_a_wall_stops_at_it() {
        let scene = Scene { solid: |c: IVec3| c.y < 0 || (c.y <= 1 && c.x >= 2), water: None };
        let (body, inside) = run(&scene, at(0.5, 0.0, 0.5), EAST, 2.0, 60.0);
        assert!(!inside);
        assert!((body.position.x - 1.7).abs() < 1e-3);
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
    fn swimmers_cannot_climb_a_bank_a_block_above_the_water() {
        let scene = Scene { solid: |c: IVec3| c.y < -2 || (c.y < 1 && c.x >= 2), water: Some(0.0) };
        let (body, _) = run(&scene, at(1.0, -1.6, 0.5), EAST_JUMPING, 2.0, 60.0);
        assert!(body.position.x < 2.0, "climbed a full block out of water");
    }

    #[test]
    fn waders_jump_out_onto_a_block() {
        // Water a third of a block deep, and a block-high bank ahead.
        let scene = Scene { solid: |c: IVec3| c.y < 0 || (c.y == 0 && c.x >= 2), water: Some(0.3) };
        let (body, inside) = run(&scene, Body { on_ground: true, ..at(1.0, 0.0, 0.5) }, EAST_JUMPING, 1.5, 60.0);
        assert!(!inside);
        assert!(body.position.x > 2.5 && body.position.y >= 1.0 - 1e-3, "{}", body.position);
    }
}
