//! How open to the sky the camera is, which decides whether mist shows around it.
//!
//! Ground fog, the weather's mist and the puffs that drift in it belong to the open air.
//! Dug down into the ground, in a cave or in a room, the camera is still below the fog's
//! top, and the sky light there says nothing: a shaft dug straight down keeps full sky
//! light the whole way, as the engine lights it. So the camera looks for the sky instead,
//! as an eye does: a fixed fan of rays over the upper hemisphere, the share that reach
//! [`REACH`] blocks without meeting a block that hides the sky. Open land and a valley's
//! floor see nearly all of them (a cliff beside it hides half at most); a forest sees
//! through its leaves; a one-block hole sees a third, a shaft three deep and a cave
//! mouth's inside next to nothing. Each pack says between which shares the mist fades
//! (`ShaderPack::mist_sky`).

use bevy::prelude::*;
use voxmar_engine::block::RenderKind;

use crate::world::ClientWorld;

/// Rays over the upper hemisphere, spread evenly by solid angle.
const RAYS: u32 = 32;
/// How far a ray looks for a roof, in blocks, and the step it takes (under a block, so it
/// does not slip between two that meet at a corner).
const REACH: f32 = 32.0;
const STEP: f32 = 0.4;
/// Seconds between looks, and the time the shown value takes to settle on a new one
/// (about two thirds of the way), so stepping into a cave mouth fades the mist out.
const EVERY: f32 = 0.1;
const SETTLE: f32 = 0.6;

pub struct ExposurePlugin;

impl Plugin for ExposurePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkyOpen>().add_systems(Update, measure);
    }
}

/// The share of the upper hemisphere above the camera that is open sky, 0..1, eased.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct SkyOpen {
    /// What is shown now.
    pub shown: f32,
    /// What the last look found.
    target: f32,
    /// Whether any look has been made: the first snaps, so a world joined in a cave does
    /// not open with mist that fades.
    looked: bool,
}

impl Default for SkyOpen {
    fn default() -> Self {
        Self { shown: 1.0, target: 1.0, looked: false }
    }
}

impl SkyOpen {
    /// Moves the shown value towards the last look over `dt` seconds.
    fn ease(&mut self, dt: f32) {
        self.shown += (self.target - self.shown) * (1.0 - (-dt / SETTLE).exp());
    }

    /// Takes a new look: the first is shown at once.
    fn see(&mut self, open: f32) {
        self.target = open;
        if !self.looked {
            self.looked = true;
            self.shown = open;
        }
    }
}

/// Looks every [`EVERY`] seconds from the camera and eases the result.
fn measure(
    time: Res<Time>,
    world: Res<ClientWorld>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut open: ResMut<SkyOpen>,
    mut since: Local<f32>,
) {
    let dt = time.delta_secs();
    *since += dt;
    if *since >= EVERY
        && let Ok(camera) = camera.single()
    {
        *since = 0.0;
        let hides = |cell: IVec3| {
            world.block(cell).is_some_and(|block| {
                let info = world.registry.info(block);
                // Leaves let the sky through, as the air under a canopy is outdoors.
                info.solid && info.render != RenderKind::Leaves
            })
        };
        open.see(sky_share(camera.translation(), hides));
    }
    let next = {
        let mut next = *open;
        next.ease(dt);
        next
    };
    open.set_if_neq(next);
}

/// The share of the upper hemisphere above `eye` that is open sky, given which cells
/// `hides` it. Unloaded space hides nothing, as the sky light rule has it.
pub fn sky_share(eye: Vec3, hides: impl Fn(IVec3) -> bool) -> f32 {
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    let mut open = 0;
    for i in 0..RAYS {
        // Equal areas of the hemisphere: the height is even, the turn the golden angle.
        let up = (i as f32 + 0.5) / RAYS as f32;
        let turn = i as f32 * golden;
        let across = (1.0 - up * up).sqrt();
        let direction = Vec3::new(across * turn.cos(), up, across * turn.sin());
        let mut distance = STEP;
        let mut reaches = true;
        while distance <= REACH {
            if hides((eye + direction * distance).floor().as_ivec3()) {
                reaches = false;
                break;
            }
            distance += STEP;
        }
        open += reaches as u32;
    }
    open as f32 / RAYS as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const EYE: Vec3 = Vec3::new(0.5, 70.6, 0.5);

    /// Solid ground up to y = 69.
    fn field(cell: IVec3) -> bool {
        cell.y < 70
    }

    #[test]
    fn open_land_sees_all_the_sky() {
        assert_eq!(sky_share(EYE, field), 1.0);
    }

    #[test]
    fn a_shaft_dug_down_sees_next_to_none() {
        // One block wide, ten deep, open to the sky: the sky light in it is 15 all the
        // way down, and the mist must still be gone.
        let shaft = |cell: IVec3| field(cell) && !(cell.x == 0 && cell.z == 0 && cell.y >= 60);
        let down = Vec3::new(0.5, 61.6, 0.5);
        let share = sky_share(down, shaft);
        assert!(share < 0.05, "{share}");
        // Three deep and three wide, the eye a block and a half under the rim: a third of the
        // sky at most, more than the shaft's nothing.
        let pit = |cell: IVec3| field(cell) && !(cell.x.abs() <= 1 && cell.z.abs() <= 1 && cell.y >= 67);
        let wide = sky_share(Vec3::new(0.5, 68.6, 0.5), pit);
        assert!(wide > share && wide < 0.45, "{wide}");
    }

    #[test]
    fn a_hole_a_block_deep_is_partly_open() {
        let hole = |cell: IVec3| field(cell) && !(cell == IVec3::new(0, 69, 0));
        let share = sky_share(Vec3::new(0.5, 69.6, 0.5), hole);
        assert!((0.1..0.9).contains(&share), "{share}");
    }

    #[test]
    fn a_wall_beside_open_ground_hides_half_at_most() {
        // A cliff face a block away, ten high: half the sky, not enough to dim a valley's fog.
        let cliff = |cell: IVec3| field(cell) || (cell.x >= 1 && cell.y < 80);
        let share = sky_share(EYE, cliff);
        assert!((0.4..=0.6).contains(&share), "{share}");
    }

    #[test]
    fn a_roofed_room_sees_none_and_leaves_let_the_sky_through() {
        let room = |cell: IVec3| field(cell) || cell.y >= 74 || cell.x.abs() > 3 || cell.z.abs() > 3;
        assert_eq!(sky_share(EYE, room), 0.0);
        // Leaves are not roofs: the caller does not count them, so a canopy is open.
        let canopy = |cell: IVec3| field(cell);
        assert_eq!(sky_share(EYE, canopy), 1.0);
    }

    #[test]
    fn unloaded_space_hides_nothing_and_the_first_look_is_shown_at_once() {
        assert_eq!(sky_share(EYE, |_| false), 1.0);
        let mut open = SkyOpen::default();
        open.see(0.0);
        assert_eq!(open.shown, 0.0);
        open.see(1.0);
        assert_eq!(open.shown, 0.0, "later looks ease in");
        for _ in 0..20 {
            open.ease(0.1);
        }
        assert!(open.shown > 0.95, "{}", open.shown);
    }
}
