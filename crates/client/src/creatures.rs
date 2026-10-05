//! Creatures on the client: what the server says lives nearby, drawn as box models from
//! their species' body plan (`engine::creature::Model`), carried on smoothly between the
//! server's updates, and animated from how they move: legs swing with their stride, heads
//! dip to graze and lift when alert, tails sway, wings flap, a struck creature flashes red
//! and a dead one topples and sinks away. Left click on one strikes it (`core:attack`).
//!
//! They are ordinary Bevy meshes outside the renderer (docs/rendering.md, "Keeping room for
//! what comes next"), unlit, with their faces shaded by hand and their colours darkened by
//! the light where they stand (sky and block light, as dropped items take theirs). A lit,
//! instanced creature pass belongs to the graphics thread when it comes.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_engine::action::{ActionTarget, REACH_CREATURE};
use voxmar_engine::creature::brain::Pose;
use voxmar_engine::creature::{self, EYE, Part, Swing};
use voxmar_engine::physics::Aabb;
use voxmar_protocol::{CreatureGone, CreatureView};

use crate::Actions;
use crate::breaking::sky_light;
use crate::drops::block_light;
use crate::interact::Target;
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::world::ClientWorld;

/// How quickly the shown position catches up with where the creature should be, per
/// second, and the furthest it runs ahead of the server's last word, in seconds.
const FOLLOW: f32 = 14.0;
const AHEAD: f32 = 0.25;
/// Seconds a struck creature flashes red, and a dead one takes to topple and go.
const FLASH: f32 = 0.3;
const DYING: f32 = 0.9;
/// Creatures further than this are not drawn.
const DRAWN_WITHIN: f32 = 96.0;
/// The darkest a creature is drawn, so one in a cave is still a shape.
const DARKEST: f32 = 0.12;

pub struct CreaturesPlugin;

impl Plugin for CreaturesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Creatures>()
            .add_systems(Update, show.run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut commands: Commands, mut creatures: ResMut<Creatures>| {
                for shown in creatures.shown.values() {
                    commands.entity(shown.entity).despawn();
                }
                *creatures = Creatures::default();
            });
    }
}

/// A creature as the client shows it.
struct Shown {
    view: CreatureView,
    /// When the view arrived, in seconds since startup.
    told_at: f32,
    position: Vec3,
    yaw: f32,
    entity: Entity,
    parts: Vec<(Entity, Part)>,
    /// Its materials, one per colour (and one for its eyes), to darken and flash.
    materials: Vec<(Handle<StandardMaterial>, Color)>,
    /// Stride phase, in cycles.
    stride: f32,
    flash: f32,
    /// Seconds since it died, if it did.
    dying: Option<f32>,
    /// Smoothed head pitch and body crouch.
    nod: f32,
    crouch: f32,
}

/// Every creature the server has told us about.
#[derive(Resource, Default)]
pub struct Creatures {
    shown: FxHashMap<u32, Shown>,
    /// Views and departures received since they were last applied.
    inbox: Vec<CreatureView>,
    left: Vec<CreatureGone>,
    /// A cube with its faces shaded, shared by every part.
    cube: Option<Handle<Mesh>>,
    /// Creatures seen to die since joining.
    pub deaths: u32,
}

impl Creatures {
    /// Takes what the server sent.
    pub fn receive(&mut self, changed: Vec<CreatureView>, gone: Vec<CreatureGone>) {
        self.inbox.extend(changed);
        self.left.extend(gone);
    }

    /// How many creatures are shown, alive.
    pub fn alive(&self) -> usize {
        self.shown.values().filter(|s| s.dying.is_none()).count()
    }

    /// The creatures shown alive, by id: species and where they stand.
    pub fn views(&self) -> impl Iterator<Item = &CreatureView> {
        self.shown.values().filter(|s| s.dying.is_none()).map(|s| &s.view)
    }

    /// The box a creature is drawn in, by its species' size and its own scale.
    fn aabb(&self, launch: &Launch, id: u32) -> Option<Aabb> {
        let shown = self.shown.get(&id).filter(|s| s.dying.is_none())?;
        let size = launch.content.creatures.get(shown.view.species).def.body.size;
        Some(Aabb::standing(shown.position, size.0 * shown.view.scale, size.1 * shown.view.scale))
    }
}

/// A unit cube centred on the origin whose faces are shaded as the sun would light them
/// from above: top brightest, then the sides, the bottom darkest.
fn shaded_cube() -> Mesh {
    // (normal, shade, four corners)
    let faces: [([f32; 3], f32, [[f32; 3]; 4]); 6] = [
        ([0.0, 1.0, 0.0], 1.0, [[-0.5, 0.5, -0.5], [-0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]]),
        ([0.0, -1.0, 0.0], 0.5, [[-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, -0.5, 0.5], [-0.5, -0.5, 0.5]]),
        ([1.0, 0.0, 0.0], 0.8, [[0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5], [0.5, -0.5, 0.5]]),
        ([-1.0, 0.0, 0.0], 0.7, [[-0.5, -0.5, -0.5], [-0.5, -0.5, 0.5], [-0.5, 0.5, 0.5], [-0.5, 0.5, -0.5]]),
        ([0.0, 0.0, 1.0], 0.75, [[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]]),
        ([0.0, 0.0, -1.0], 0.85, [[-0.5, -0.5, -0.5], [-0.5, 0.5, -0.5], [0.5, 0.5, -0.5], [0.5, -0.5, -0.5]]),
    ];
    let (mut positions, mut normals, mut colours, mut indices) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (normal, shade, corners) in faces {
        let base = positions.len() as u32;
        for corner in corners {
            positions.push(corner);
            normals.push(normal);
            colours.push([shade, shade, shade, 1.0]);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
        .with_inserted_indices(Indices::U32(indices))
}

/// A colour from the species' palette, a shade lighter or darker by the individual.
fn tint(rgb: [u8; 3], seed: u32) -> Color {
    let shift = 0.92 + 0.16 * creature::unit_from(creature::mix(seed as u64, 9));
    Color::srgb(rgb[0] as f32 / 255.0 * shift, rgb[1] as f32 / 255.0 * shift, rgb[2] as f32 / 255.0 * shift)
}

/// Applies what the server sent, then moves and animates every creature, and handles
/// aiming at and striking them.
#[allow(clippy::too_many_arguments)]
fn show(
    mut commands: Commands,
    time: Res<Time>,
    launch: Res<Launch>,
    world: Res<ClientWorld>,
    cycle: Res<crate::render::DayCycle>,
    player: Res<LocalPlayer>,
    mut creatures: ResMut<Creatures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut transforms: Query<&mut Transform>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let cube = creatures.cube.get_or_insert_with(|| meshes.add(shaded_cube())).clone();
    let registry = &launch.content.creatures;

    let creatures = &mut *creatures;
    for gone in std::mem::take(&mut creatures.left) {
        match creatures.shown.get_mut(&gone.id) {
            Some(shown) if gone.died => {
                shown.dying = Some(0.0);
                creatures.deaths += 1;
            }
            Some(shown) => {
                commands.entity(shown.entity).despawn();
                creatures.shown.remove(&gone.id);
            }
            None => {}
        }
    }
    for view in std::mem::take(&mut creatures.inbox) {
        if let Some(shown) = creatures.shown.get_mut(&view.id) {
            if view.health < shown.view.health {
                shown.flash = FLASH;
            }
            shown.view = view;
            shown.told_at = now;
            continue;
        }
        if view.species.0 as usize >= registry.len() {
            continue;
        }
        let species = registry.get(view.species);
        let def = &species.def;
        let glows = def.body.glows;
        let eye = if glows { Color::WHITE } else { Color::srgb(0.06, 0.06, 0.07) };
        let palette: Vec<(Handle<StandardMaterial>, Color)> = def
            .body
            .colours
            .iter()
            .map(|&rgb| tint(rgb, view.seed))
            .chain([eye])
            .map(|colour| (materials.add(StandardMaterial { base_color: colour, unlit: true, ..default() }), colour))
            .collect();
        let entity = commands
            .spawn((Transform::from_translation(view.position), Visibility::default(), DespawnOnExit(Screen::InGame)))
            .id();
        let parts: Vec<(Entity, Part)> = species
            .parts
            .iter()
            .map(|&part| {
                let index = if part.colour == EYE { palette.len() - 1 } else { (part.colour as usize).min(palette.len() - 2) };
                let child = commands
                    .spawn((Mesh3d(cube.clone()), MeshMaterial3d(palette[index].0.clone()), Transform::from_translation(part.centre).with_scale(part.size), ChildOf(entity)))
                    .id();
                (child, part)
            })
            .collect();
        creatures.shown.insert(
            view.id,
            Shown { view, told_at: now, position: view.position, yaw: view.yaw, entity, parts, materials: palette, stride: 0.0, flash: 0.0, dying: None, nod: 0.0, crouch: 0.0 },
        );
    }

    let daylight = 0.25 + 0.75 * creature::daylight(cycle.ticks.max(0.0) as u64);
    let eye = player.eye();
    let mut finished = Vec::new();
    for (&id, shown) in creatures.shown.iter_mut() {
        let def = &registry.get(shown.view.species).def;
        // Carry on along its velocity a little past the last word, then ease toward it.
        let ahead = (now - shown.told_at).min(AHEAD);
        let target = shown.view.position + shown.view.velocity.with_y(0.0) * ahead;
        let before = shown.position;
        shown.position = shown.position.lerp(target, (FOLLOW * dt).min(1.0));
        if shown.position.distance(target) > 6.0 {
            shown.position = target;
        }
        let turn = (shown.view.yaw - shown.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        shown.yaw += turn * (10.0 * dt).min(1.0);
        let speed = (shown.position - before).with_y(0.0).length() / dt.max(1e-4);
        let scale = shown.view.scale;
        shown.stride += speed * dt / (def.body.size.0.max(0.3) * 1.6 * scale);
        shown.flash = (shown.flash - dt).max(0.0);

        let (nod, crouch) = match shown.view.pose {
            Pose::Grazing => (0.8, 0.0),
            Pose::Alert => (-0.25, 0.0),
            Pose::Resting => (0.35, 0.18),
            Pose::Crouched => (0.1, 0.25),
            Pose::Attacking => (0.3, 0.05),
            Pose::Idle => (0.0, 0.0),
        };
        shown.nod += (nod - shown.nod) * (6.0 * dt).min(1.0);
        shown.crouch += (crouch - shown.crouch) * (6.0 * dt).min(1.0);

        let mut root = Transform::from_translation(shown.position - Vec3::Y * shown.crouch * def.body.size.1 * scale)
            .with_rotation(Quat::from_rotation_y(shown.yaw))
            .with_scale(Vec3::splat(scale));
        if let Some(dying) = shown.dying.as_mut() {
            *dying += dt;
            let t = (*dying / DYING).min(1.0);
            root.rotation *= Quat::from_rotation_z(t.min(0.5) * 2.0 * std::f32::consts::FRAC_PI_2);
            root.translation.y -= (t - 0.5).max(0.0) * def.body.size.1 * scale;
            if t >= 1.0 {
                finished.push(id);
            }
        }
        if let Ok(mut transform) = transforms.get_mut(shown.entity) {
            *transform = root;
        }
        let far = shown.position.distance(eye) > DRAWN_WITHIN;
        let swing = (speed / (def.movement.walk.max(0.5) * 1.5)).min(1.0);
        for (child, part) in &shown.parts {
            let rotation = match part.swing {
                Swing::Leg { phase } => Quat::from_rotation_x(((shown.stride + phase) * std::f32::consts::TAU).sin() * 0.7 * swing),
                Swing::Head => Quat::from_rotation_x(-shown.nod),
                Swing::Tail => Quat::from_rotation_y((now * 2.5 + id as f32).sin() * 0.3 + speed.min(4.0) * 0.05),
                Swing::Wing { side } => {
                    let beat = if speed > def.movement.walk * 1.2 { (now * 18.0).sin() * 0.5 } else { 0.0 };
                    Quat::from_rotation_z(side * beat)
                }
                Swing::Still => Quat::IDENTITY,
            };
            if let Ok(mut transform) = transforms.get_mut(*child) {
                transform.translation = part.pivot + rotation * (part.centre - part.pivot);
                transform.rotation = rotation;
                transform.scale = if far { Vec3::ZERO } else { part.size };
            }
        }
        // Lit as the terrain around it is, a glowing creature by itself; red when struck.
        let cell = (shown.position + Vec3::Y * 0.5).floor().as_ivec3();
        let light = if def.body.glows {
            1.0
        } else {
            let sky = sky_light(&world, cell) as f32 / 15.0 * daylight;
            let block = block_light(&world, cell) as f32 / 15.0;
            sky.max(block).max(DARKEST)
        };
        let red = shown.flash / FLASH;
        for (handle, colour) in &shown.materials {
            if let Some(mut material) = materials.get_mut(handle) {
                let lit = colour.to_linear() * light;
                material.base_color = Color::LinearRgba(lit.mix(&LinearRgba::rgb(0.9, 0.08, 0.05), red * 0.7));
            }
        }
    }
    for id in finished {
        if let Some(shown) = creatures.shown.remove(&id) {
            commands.entity(shown.entity).despawn();
        }
    }
}

/// Finds the creature under the crosshair, nearer than any block, and strikes it on a
/// left click. Runs between finding the block target and breaking, so aiming at a
/// creature never mines the ground behind it.
pub fn aim(
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    launch: Res<Launch>,
    net: Res<Net>,
    actions: Res<Actions>,
    player: Res<LocalPlayer>,
    creatures: Res<Creatures>,
    mut target: ResMut<Target>,
) {
    let eye = player.eye();
    let look = player.look_direction().normalize_or_zero();
    let block = target.hit.map_or(f32::MAX, |hit| hit.distance);
    let nearest = creatures
        .shown
        .keys()
        .filter_map(|&id| Some((id, ray_box(eye, look, &creatures.aabb(&launch, id)?)?)))
        .filter(|&(_, d)| d <= REACH_CREATURE && d < block)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    target.creature = nearest.map(|(id, _)| id);
    if target.creature.is_some() {
        target.hit = None;
    }
    if let Some(id) = target.creature
        && ui.cursor_locked
        && !player.dead
        && mouse.just_pressed(MouseButton::Left)
        && let Some(attack) = actions.0.id("core:attack")
    {
        net.propose_from(&player, attack, ActionTarget::Creature(id));
    }
}

/// Where a ray from `origin` along unit `direction` first enters `aabb`, if it does.
fn ray_box(origin: Vec3, direction: Vec3, aabb: &Aabb) -> Option<f32> {
    let inverse = direction.recip();
    let t1 = (aabb.min - origin) * inverse;
    let t2 = (aabb.max - origin) * inverse;
    let near = t1.min(t2).max_element();
    let far = t1.max(t2).min_element();
    (far >= near.max(0.0)).then_some(near.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_find_boxes_in_front_only() {
        let aabb = Aabb { min: Vec3::new(2.0, -0.5, -0.5), max: Vec3::new(3.0, 0.5, 0.5) };
        assert_eq!(ray_box(Vec3::ZERO, Vec3::X, &aabb), Some(2.0));
        assert_eq!(ray_box(Vec3::ZERO, Vec3::NEG_X, &aabb), None);
        assert_eq!(ray_box(Vec3::ZERO, Vec3::Y, &aabb), None);
    }
}
