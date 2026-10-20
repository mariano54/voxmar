//! Other players, drawn as people (`player_look.ron`): the same boxes as a villager, skinned,
//! painted and lit as creatures are (`creatures::SkinMaterial`), in colours their name gives
//! them, so a friend looks the same every time they come. Each follows where the server says
//! they are, eased in; legs and arms swing with the ground covered, the head looks up and
//! down as they do, and what is in their hand is held in their right fist, drawn as dropped
//! items are (`render::items`, held still).

use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use bevy::tasks::{Task, block_on, futures_lite::future};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use voxmar_engine::creature::skin::{self, Marking};
use voxmar_engine::creature::{self, BodyDef, Part, Role, Swing};
use voxmar_engine::content::ContentFile;
use voxmar_protocol::PlayerId;

use crate::breaking::sky_light;
use crate::creatures::{CreatureSkin, SkinFrame, SkinMaterial, SpeciesBody};
use crate::drops::block_light;
use crate::menu::Screen;
use crate::net::RemotePlayers;
use crate::render::items::{ItemModel, ItemModels, Shape};
use crate::world::ClientWorld;

pub const FILE: &str = "player_look.ron";

/// Further than this from where the server says they are, a player is shown there at once
/// (a teleport) rather than eased toward it.
const SNAP: f32 = 8.0;

/// How other players look and move (`player_look.ron`).
#[derive(Resource, Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerLook {
    pub body: BodyDef,
    pub skins: Vec<[u8; 3]>,
    pub shirts: Vec<[u8; 3]>,
    pub trousers: Vec<[u8; 3]>,
    pub hair: Vec<[u8; 3]>,
    pub follow: f32,
    pub turn: f32,
    pub stride: f32,
    pub swing: f32,
    pub full_pace: f32,
    pub look: f32,
    pub look_most: f32,
    pub carry: f32,
    pub carry_swing: f32,
    pub hand: Hand,
}

/// What is in a player's hand: where its middle sits from their right shoulder, in the
/// arm's frame; how it is turned there (degrees about x, y and z, in that order); and its
/// width, flat or a block.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hand {
    pub at: (f32, f32, f32),
    pub turn: (f32, f32, f32),
    pub flat: f32,
    pub block: f32,
}

/// The colour indices each player's own replace: shirt, trousers and hair (`player_look.ron`).
const SHIRT: usize = 0;
const TROUSERS: usize = 1;
const HAIR: usize = 3;

impl PlayerLook {
    /// Reads every package's file in order, the last one giving the look, and refuses one
    /// that cannot be drawn.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut look = None;
        for file in files.iter().filter(|f| f.name == FILE) {
            look = Some(ron::from_str::<PlayerLook>(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?);
        }
        let look = look.ok_or(format!("no package gives {FILE}"))?;
        look.check().map_err(|e| format!("{FILE}: {e}"))?;
        Ok(look)
    }

    pub fn check(&self) -> Result<(), String> {
        let parts = self.parts();
        let colours = self.body.colours.len();
        if let Some(part) = parts.iter().find(|p| p.colour as usize >= colours) {
            return Err(format!("a {:?} box takes colour {}, of {colours}", part.role, part.colour));
        }
        if colours <= HAIR {
            return Err(format!("the body needs {} colours (shirt, trousers, belt, hair), not {colours}", HAIR + 1));
        }
        if !parts.iter().any(|p| p.role == Role::Head) {
            return Err("the body has no head".into());
        }
        if !parts.iter().any(|p| p.role == Role::Leg && matches!(p.swing, Swing::Leg { .. })) {
            return Err("the body has no legs that swing".into());
        }
        if !parts.iter().any(|p| right_arm(p) && p.role == Role::Arm) {
            return Err("the body has no right arm (an Arm box right of the middle, +x) to hold things in".into());
        }
        for (name, list) in [("skins", &self.skins), ("shirts", &self.shirts), ("trousers", &self.trousers), ("hair", &self.hair)] {
            if list.is_empty() {
                return Err(format!("`{name}` is empty"));
            }
        }
        let numbers = [
            ("follow", self.follow, 1.0, 60.0),
            ("turn", self.turn, 1.0, 60.0),
            ("stride", self.stride, 0.5, 5.0),
            ("swing", self.swing, 0.0, 80.0),
            ("full_pace", self.full_pace, 0.5, 20.0),
            ("look", self.look, 0.0, 1.0),
            ("look_most", self.look_most, 0.0, 90.0),
            ("carry", self.carry, -45.0, 90.0),
            ("carry_swing", self.carry_swing, 0.0, 1.0),
            ("hand.flat", self.hand.flat, 0.05, 1.5),
            ("hand.block", self.hand.block, 0.05, 1.5),
        ];
        for (name, value, least, most) in numbers {
            if !(least..=most).contains(&value) {
                return Err(format!("`{name}` must be {least} to {most}, not {value}"));
            }
        }
        let (x, y, z) = self.hand.at;
        if !(x.is_finite() && y.is_finite() && z.is_finite()) || Vec3::new(x, y, z).length() > 2.0 {
            return Err(format!("`hand.at` must be within 2 blocks of the shoulder, not {:?}", self.hand.at));
        }
        Ok(())
    }

    /// The body's boxes.
    pub fn parts(&self) -> Vec<Part> {
        self.body.model.parts(self.body.scale)
    }

    /// The body `name` is drawn with: their skin, shirt, trousers and hair.
    pub fn body_of(&self, name: &str) -> BodyDef {
        let pick = |list: &[[u8; 3]], salt: u64| list[(creature::mix(name_hash(name), salt) % list.len() as u64) as usize];
        let mut body = self.body.clone();
        body.colours[SHIRT] = pick(&self.shirts, 1);
        body.colours[TROUSERS] = pick(&self.trousers, 2);
        body.colours[HAIR] = pick(&self.hair, 3);
        body.skin.markings.push(Marking::Hood { colour: pick(&self.skins, 4) });
        body
    }
}

/// A name's own number, the same on every machine.
fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Whether a box is of the right arm: what holds things.
fn right_arm(part: &Part) -> bool {
    matches!(part.role, Role::Arm | Role::Neck) && part.pivot.x > 0.0
}

/// One other player as drawn.
struct Figure {
    root: Entity,
    joints: Vec<(Entity, Part)>,
    material: Handle<SkinMaterial>,
    painting: Option<Task<Image>>,
    skin: Handle<Image>,
    position: Vec3,
    yaw: f32,
    pitch: f32,
    /// Strides taken, and how fast they go (blocks a second), eased.
    stride: f32,
    pace: f32,
    /// What it shows now, for the gate: the right arm's swing (radians), the left leg's,
    /// and where the middle of what is in its hand is.
    arm: f32,
    leg: f32,
    head: f32,
    held_at: Option<Vec3>,
}

/// Other players as drawn, by id.
#[derive(Resource, Default)]
pub struct Figures {
    /// The gate's fixed step of their time each frame, in place of the frame's.
    pub step: Option<f32>,
    body: Option<SpeciesBody>,
    blank: Option<Handle<Image>>,
    shown: FxHashMap<PlayerId, Figure>,
}

/// What a figure shows, for the gate.
#[derive(Clone, Copy, Debug)]
pub struct FigureRead {
    pub position: Vec3,
    pub yaw: f32,
    /// Whether its skin is painted and it is drawn.
    pub painted: bool,
    pub joints: usize,
    /// How far its right arm and its left leg are swung from hanging, radians (forward
    /// positive).
    pub arm: f32,
    pub leg: f32,
    /// How far its head is tilted up (positive) or down, radians.
    pub head: f32,
    /// Where the middle of what is in its hand is drawn, if anything is.
    pub held_at: Option<Vec3>,
}

impl Figures {
    /// Where another player's body is drawn, once it is (what the eye sees of them, eased
    /// behind where the server says they are).
    pub fn drawn_at(&self, id: PlayerId) -> Option<Vec3> {
        self.shown.get(&id).filter(|f| f.painting.is_none()).map(|f| f.position)
    }

    pub fn read(&self, id: PlayerId) -> Option<FigureRead> {
        let f = self.shown.get(&id)?;
        Some(FigureRead { position: f.position, yaw: f.yaw, painted: f.painting.is_none(), joints: f.joints.len(), arm: f.arm, leg: f.leg, head: f.head, held_at: f.held_at })
    }
}

pub struct FiguresPlugin;

impl Plugin for FiguresPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Figures>()
            .add_systems(Update, show.after(crate::drops::show).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut figures: ResMut<Figures>, mut skins: ResMut<Assets<SkinMaterial>>, mut images: ResMut<Assets<Image>>| {
                for (_, figure) in figures.shown.drain() {
                    forget(&figure, &mut skins, &mut images);
                }
            });
    }
}

/// Lets a figure's material and painted skin go (the blank one all share stays).
fn forget(figure: &Figure, skins: &mut Assets<SkinMaterial>, images: &mut Assets<Image>) {
    skins.remove(&figure.material);
    if figure.painting.is_none() {
        images.remove(&figure.skin);
    }
}

/// The turn about x by `degrees`.
fn about_x(degrees: f32) -> Quat {
    Quat::from_rotation_x(degrees.to_radians())
}

/// Eases `from` toward `to` at `rate` a second, the short way round.
fn ease_angle(from: f32, to: f32, rate: f32, dt: f32) -> f32 {
    let gap = (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    from + gap * (1.0 - (-rate * dt).exp())
}

/// Eases a look up or down (`pitch`, clamped, never round the circle) from `from` toward
/// `to` at `rate` a second: straight through the level, never over the top.
fn ease_pitch(from: f32, to: f32, rate: f32, dt: f32) -> f32 {
    from + (to - from) * (1.0 - (-rate * dt).exp())
}

#[allow(clippy::too_many_arguments)]
/// Where a figure at `from` is after `dt` seconds following the server's `to` at `rate`,
/// and how far it walked: from more than `SNAP` off it is there at once, and a snap (a
/// respawn, a teleport) is no step, so it swings no limb.
fn follow(from: Vec3, to: Vec3, rate: f32, dt: f32) -> (Vec3, f32) {
    if from.distance(to) > SNAP {
        return (to, 0.0);
    }
    let at = from.lerp(to, 1.0 - (-rate * dt).exp());
    (at, (at - from).with_y(0.0).length())
}

/// Spawns a figure's root at `at`, a joint per part and its skinned body (which casts no
/// shadow): the root, the joints and the body.
fn spawn_figure(commands: &mut Commands, body: &SpeciesBody, parts: &[Part], material: Handle<SkinMaterial>, at: Vec3, visibility: Visibility) -> (Entity, Vec<(Entity, Part)>, Entity) {
    let root = commands.spawn((Transform::from_translation(at), visibility, DespawnOnExit(Screen::InGame))).id();
    let joints: Vec<(Entity, Part)> = parts.iter().map(|&part| (commands.spawn((Transform::from_translation(part.pivot), ChildOf(root))).id(), part)).collect();
    let skinned = SkinnedMesh { inverse_bindposes: body.binds.clone(), joints: joints.iter().map(|(e, _)| *e).collect() };
    let mesh = commands.spawn((Mesh3d(body.mesh.clone()), MeshMaterial3d(material), skinned, Transform::default(), Visibility::default(), NotShadowCaster, ChildOf(root))).id();
    (root, joints, mesh)
}

/// A figure's material: unlit, its skin painted through `body`'s volume, `skin` till then.
fn figure_material(body: &SpeciesBody, skin: Handle<Image>, atmosphere: Buffer) -> SkinMaterial {
    let size = Vec3::from_array(body.dims.map(|n| n as f32)) / skin::TEXELS;
    let frame = SkinFrame { origin: body.origin.extend(skin::TEXELS), size: size.extend(0.0) };
    SkinMaterial { base: StandardMaterial { unlit: true, ..default() }, extension: CreatureSkin { skin, frame, atmosphere } }
}

fn show(
    mut commands: Commands,
    time: Res<Time>,
    look: Res<PlayerLook>,
    remote: Res<RemotePlayers>,
    world: Res<ClientWorld>,
    pieces: Option<Res<crate::render::particles::TexturePieces>>,
    light: Res<crate::render::SceneLight>,
    atmosphere: Res<crate::render::atmosphere::SharedAtmosphere>,
    mut figures: ResMut<Figures>,
    mut models: ResMut<ItemModels>,
    mut assets: (ResMut<Assets<Mesh>>, ResMut<Assets<SkinnedMeshInverseBindposes>>, ResMut<Assets<SkinMaterial>>, ResMut<Assets<Image>>),
    mut transforms: Query<&mut Transform>,
    mut visible: Query<&mut Visibility>,
) {
    let (meshes, binds, skins, images) = (&mut assets.0, &mut assets.1, &mut assets.2, &mut assets.3);
    let Some(atmosphere) = atmosphere.get() else { return };
    let dt = figures.step.unwrap_or(time.delta_secs());
    let figures = &mut *figures;
    // Gone players' figures go.
    figures.shown.retain(|&id, figure| {
        let here = remote.views().any(|view| view.id == id);
        if !here {
            commands.entity(figure.root).despawn();
            forget(figure, skins, images);
        }
        here
    });
    let parts = look.parts();
    let blank = figures.blank.get_or_insert_with(|| images.add(crate::creatures::blank_skin())).clone();
    let body = figures.body.get_or_insert_with(|| {
        let body = crate::creatures::species_body(&look.body.model, &parts, meshes, binds);
        // A figure drawn as another player's is, joints and all, kept a speck at the eye
        // from the world's load (`render::warm`): its pipelines compile now, not when
        // another player first comes into view.
        let material = skins.add(figure_material(&body, blank.clone(), atmosphere.clone()));
        let (root, _, mesh) = spawn_figure(&mut commands, &body, &parts, material, Vec3::ZERO, Visibility::Inherited);
        commands.entity(mesh).insert(NoFrustumCulling);
        crate::render::warm::at_eye(&mut commands, root);
        body
    });
    let hand = look.hand;
    for view in remote.views() {
        let figure = figures.shown.entry(view.id).or_insert_with(|| {
            let material = skins.add(figure_material(body, blank.clone(), atmosphere.clone()));
            let (root, joints, _) = spawn_figure(&mut commands, body, &parts, material.clone(), view.position, Visibility::Hidden);
            let seed = name_hash(&view.name) as u32;
            let painting = Some(crate::creatures::paint_skin(look.body_of(&view.name), parts.clone(), body, seed, false));
            Figure { root, joints, material, painting, skin: blank.clone(), position: view.position, yaw: view.yaw, pitch: view.pitch, stride: 0.0, pace: 0.0, arm: 0.0, leg: 0.0, head: 0.0, held_at: None }
        });
        // Its skin, once painted; till then it is not drawn.
        if let Some(task) = figure.painting.as_mut()
            && let Some(image) = block_on(future::poll_once(task))
        {
            figure.painting = None;
            figure.skin = images.add(image);
            if let Some(mut material) = skins.get_mut(&figure.material) {
                material.extension.skin = figure.skin.clone();
            }
            if let Ok(mut visibility) = visible.get_mut(figure.root) {
                *visibility = Visibility::Inherited;
            }
        }
        // Where it is: eased toward what the server says, or there at once from far off.
        let moved;
        (figure.position, moved) = follow(figure.position, view.position, look.follow, dt);
        figure.yaw = ease_angle(figure.yaw, view.yaw, look.turn, dt);
        figure.pitch = ease_pitch(figure.pitch, view.pitch, look.turn, dt);
        if dt > 0.0 {
            figure.pace += (moved / dt - figure.pace) * (1.0 - (-10.0 * dt).exp());
        }
        figure.stride += moved / look.stride;
        let going = (figure.pace / look.full_pace).clamp(0.0, 1.0);
        let rotation = Quat::from_rotation_y(figure.yaw);
        if let Ok(mut transform) = transforms.get_mut(figure.root) {
            *transform = Transform::from_translation(figure.position).with_rotation(rotation);
        }
        // Each joint: legs and arms swing with the stride, the right arm held forward with
        // something in it, the head looks up and down.
        let holding = view.held.is_some();
        let head = (figure.pitch.to_degrees() * look.look).clamp(-look.look_most, look.look_most);
        let mut arm_turn = Quat::IDENTITY;
        let mut shoulder = Vec3::ZERO;
        for &(joint, part) in &figure.joints {
            let turn = match part.swing {
                Swing::Leg { phase, .. } => {
                    let swing = look.swing * going * ((figure.stride + phase) * std::f32::consts::TAU).sin();
                    if right_arm(&part) && holding {
                        look.carry + swing * look.carry_swing
                    } else {
                        swing
                    }
                }
                Swing::Head => head,
                _ => 0.0,
            };
            let q = about_x(turn);
            if right_arm(&part) && part.role == Role::Arm {
                (arm_turn, shoulder) = (q, part.pivot);
                figure.arm = turn.to_radians();
            }
            if part.role == Role::Leg && part.pivot.x < 0.0 {
                figure.leg = turn.to_radians();
            }
            if part.swing == Swing::Head {
                figure.head = turn.to_radians();
            }
            if let Ok(mut transform) = transforms.get_mut(joint) {
                *transform = Transform::from_translation(part.pivot).with_rotation(q);
            }
        }
        // Lit as the ground it stands on, as creatures are.
        let cell = (figure.position + Vec3::Y * 0.5).floor().as_ivec3();
        let (sky, block) = (sky_light(&world, cell), block_light(&world, cell));
        let lit = light.at(sky, block);
        let step = |c: f32| (c * crate::creatures::LIGHT_STEPS).round() / crate::creatures::LIGHT_STEPS;
        let colour = Color::LinearRgba(LinearRgba::rgb(step(lit.red), step(lit.green), step(lit.blue)));
        if skins.get(&figure.material).is_some_and(|m| m.base.base_color != colour)
            && let Some(mut material) = skins.get_mut(&figure.material)
        {
            material.base.base_color = colour;
        }
        // What is in the hand, held still in the right fist.
        figure.held_at = None;
        let Some(item) = view.held.filter(|_| figure.painting.is_none()) else { continue };
        let (x, y, z) = hand.at;
        let held = rotation * arm_turn;
        let center = figure.position + rotation * shoulder + held * Vec3::new(x, y, z);
        let (tx, ty, tz) = hand.turn;
        let turn = held * Quat::from_euler(EulerRot::XYZ, tx.to_radians(), ty.to_radians(), tz.to_radians());
        let looks = crate::drops::look_of(&world, pieces.as_deref(), item, center.floor().as_ivec3());
        let size = if looks.shape == Shape::Sprite { hand.flat } else { hand.block };
        figure.held_at = Some(center);
        // At the front: the drops before it are cut to what is drawn (`drops::show`, nearest
        // first), and what a player holds is never the one left out.
        models.0.insert(0, ItemModel {
            center,
            phase: 0.0,
            offset: Vec3::ZERO,
            size,
            shape: looks.shape,
            layers: looks.layers,
            tinted: looks.tinted,
            tint: looks.tint,
            light: [sky, block],
            held: None,
            posed: Some(turn.to_scaled_axis()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> PlayerLook {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/base/", "player_look.ron")).unwrap();
        PlayerLook::load(&[ContentFile { package: "base".into(), name: FILE.into(), source }]).unwrap()
    }

    /// A remote player who respawns or is teleported is drawn there at once, and the jump
    /// is no stride: it does not run on the spot.
    #[test]
    fn a_teleported_figure_is_there_at_once_without_a_step() {
        let from = Vec3::ZERO;
        let (at, moved) = follow(from, Vec3::new(SNAP + 40.0, 0.0, 0.0), 12.0, 1.0 / 60.0);
        assert_eq!((at, moved), (Vec3::new(SNAP + 40.0, 0.0, 0.0), 0.0), "a snap swings no limb");
        let (at, moved) = follow(from, Vec3::new(1.0, 0.0, 0.0), 12.0, 1.0 / 60.0);
        assert!(moved > 0.0 && (moved - at.x).abs() < 1e-6, "a walk is a step: {moved} to {at}");
    }

    /// A look from all but straight down to all but straight up passes the level, never
    /// over the top, though that is more than half a turn.
    #[test]
    fn a_look_up_from_down_passes_the_level() {
        let (mut pitch, mut passed) = (-1.55_f32, false);
        for _ in 0..120 {
            let next = ease_pitch(pitch, 1.55, 12.0, 1.0 / 60.0);
            assert!(next >= pitch && next <= 1.55, "{pitch} to {next}");
            passed |= next.abs() < 0.2;
            pitch = next;
        }
        assert!(passed && pitch > 1.5, "the look ended at {pitch}");
    }

    #[test]
    fn the_shipped_look_loads_and_a_broken_one_is_refused() {
        let look = shipped();
        let mut broken = look.clone();
        broken.shirts.clear();
        assert!(broken.check().unwrap_err().contains("shirts"));
        let mut broken = look.clone();
        broken.body.colours.truncate(2);
        assert!(broken.check().is_err());
        let mut broken = look.clone();
        broken.stride = 0.0;
        assert!(broken.check().unwrap_err().contains("stride"));
        let mut broken = look;
        if let creature::Model::Boxes(boxes) = &mut broken.body.model {
            boxes.retain(|b| b.role != Role::Arm);
        }
        assert!(broken.check().unwrap_err().contains("right arm"));
    }

    /// A name always gives the same look, and names differ.
    #[test]
    fn a_name_gives_its_own_look_every_time() {
        let look = shipped();
        let (ada, again, bo) = (look.body_of("Ada"), look.body_of("Ada"), look.body_of("Bo"));
        assert_eq!(ada.colours, again.colours);
        let names = ["Ada", "Bo", "Cy", "Di", "Ed", "Flo", "Gus", "Hal"];
        let shirts: std::collections::HashSet<[u8; 3]> = names.iter().map(|n| look.body_of(n).colours[SHIRT]).collect();
        assert!(shirts.len() >= 3, "eight names wore only {} shirts", shirts.len());
        assert!(bo.skin.markings.iter().any(|m| matches!(m, Marking::Hood { .. })), "no skin colour on the face");
    }
}
