//! Creatures on the client: what the server says lives nearby, drawn as smooth low-poly
//! bodies (engine `creature::volume`, Mariano's pick, 2026-10-06) built from their species'
//! parts, one mesh a species, bending at one joint a part, in a pixel skin painted through
//! the body for each one (`creature::skin::paint_volume`, `creature_skin.wgsl`). They are
//! carried on smoothly between the server's updates and animated from how they move: legs
//! swing with their stride (hoppers and bounding runners move their pairs together), the
//! body bobs with each step and breathes at rest, heads dip to graze and turn to watch a
//! player who comes near, ears twitch, tails sway (a kept creature's faster when its keeper
//! is near), wings flap, a struck creature flashes red and a dead one topples and sinks
//! away. Left click on one strikes it (`core:attack`).
//!
//! The bodies are ordinary Bevy skinned meshes outside the renderer (docs/rendering.md,
//! "Keeping room for what comes next"), unlit, shaded flat by face in their shader, their
//! colour darkened by the light where they stand (sky and block light, as dropped items
//! take theirs).

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::VertexAttributeValues;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::render::render_resource::{AsBindGroup, Buffer, ShaderType};
use bevy::shader::ShaderRef;
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use rustc_hash::FxHashMap;
use voxmar_engine::action::ActionTarget;
use voxmar_engine::creature::brain::Pose;
use voxmar_engine::creature::skin::{self, FACES};
use voxmar_engine::creature::volume::{self, BodyMesh, Shape};
use voxmar_engine::creature::{self, CreatureId, Part, Style, Swing};
use voxmar_engine::physics::Aabb;
use voxmar_protocol::{CreatureGone, CreatureView};

use crate::Actions;
use crate::creature_sounds::{CreatureSound, CreatureSounds, Kind as Sound};
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
/// Seconds a struck creature flashes red.
const FLASH: f32 = 0.3;
/// Hearts shown over a creature being tamed or ready to breed.
const HEARTS: usize = 3;
/// Blocks within which a creature turns its head to watch the player, and how far it turns.
const WATCHES_WITHIN: f32 = 9.0;
const WATCH_TURN: f32 = 1.0;

/// A small bird's flight: blocks it rises and dips each bound, beats a second, how far its
/// wings swing out from folded (radians), how much longer they reach spread, and how fast
/// one that dies in the air falls, in blocks a second.
const UNDULATE: f32 = 0.18;
const WINGBEATS: f32 = 24.0;
const WING_SPREAD: f32 = 1.35;
const WING_REACH: f32 = 1.6;
const FALLS: f32 = 6.0;
/// The most blocks one that died in the air falls before it goes anyway (over an unloaded
/// column, or the void).
const FALLS_MOST: f32 = 48.0;

/// Where a flier is in its bounding flight: its rise (−1..1) and how hard it is beating its
/// wings (0..1), climbing on a burst of wingbeats and dipping in a glide, each bird to its own
/// time.
fn flight_bound(now: f32, id: u32) -> (f32, f32) {
    let phase = (now * 0.9 + creature::unit_from(id as u64) * 7.0).fract();
    let beating = if phase < 0.55 { 1.0 } else { ((0.62 - phase) / 0.07).clamp(0.0, 1.0) };
    ((phase * std::f32::consts::TAU).sin(), beating)
}

/// The heart's pixels, a row a string: `#` is heart, `+` its highlight.
const HEART: [&str; 7] = ["         ", " ##   ## ", "#+##.####", "#+#######", " ####### ", "  #####  ", "    #    "];

/// A heart drawn in pixels, white (tinted by its material), clear around it.
fn heart_image() -> Image {
    let (w, h) = (HEART[0].len() as u32, HEART.len() as u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for row in HEART {
        for c in row.chars() {
            data.extend_from_slice(match c {
                '#' => &[255, 255, 255, 255],
                '+' | '.' => &[255, 235, 235, 255],
                _ => &[0, 0, 0, 0],
            });
        }
    }
    let extent = Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let mut image = Image::new(extent, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    image.sampler = nearest();
    image
}

/// One clear texel of skin, worn while the real one is painted.
fn blank_skin() -> Image {
    Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D3, vec![0; 4], TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
}

fn nearest() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor { mag_filter: ImageFilterMode::Nearest, min_filter: ImageFilterMode::Nearest, ..default() })
}

/// Removes a creature's model and its hearts.
fn despawn(commands: &mut Commands, shown: &Shown) {
    commands.entity(shown.entity).despawn();
    for heart in &shown.hearts {
        commands.entity(*heart).despawn();
    }
}

/// Creatures built a frame at most; more coming into view wait for the next frames.
const SPAWNS: usize = 3;
/// Creatures further than this are not drawn.
const DRAWN_WITHIN: f32 = 96.0;
/// Steps a creature's light is rounded to, so the slow change of daylight rewrites its
/// material now and then rather than every frame.
const LIGHT_STEPS: f32 = 128.0;

pub struct CreaturesPlugin;

/// The gate retains its eye fixture's CPU pixels after their upload to the GPU. Other
/// skins give up their pixels as usual, so normal play pays no memory cost for the check.
#[derive(Resource)]
pub(crate) struct InspectEyes;

impl Plugin for CreaturesPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "creature_skin.wgsl");
        app.add_plugins(MaterialPlugin::<SkinMaterial>::default())
            .init_resource::<Creatures>()
            .add_systems(Update, show.run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut commands: Commands, mut creatures: ResMut<Creatures>| {
                for shown in creatures.shown.values() {
                    despawn(&mut commands, shown);
                }
                *creatures = Creatures::default();
            });
    }
}

/// A creature's material: unlit, its colour from its own skin, painted through its body's
/// volume and snapped to the block pixel grid (`creature_skin.wgsl`).
pub type SkinMaterial = ExtendedMaterial<StandardMaterial, CreatureSkin>;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct CreatureSkin {
    #[texture(100, dimension = "3d")]
    #[sampler(101)]
    pub skin: Handle<Image>,
    #[uniform(102)]
    pub frame: SkinFrame,
    /// The air between the eye and the creature, as the terrain's fog reads it
    /// (`render::atmosphere::SharedAtmosphere`).
    #[storage(103, read_only, buffer)]
    pub atmosphere: Buffer,
}

/// Where the skin's volume lies in model space.
#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct SkinFrame {
    /// The volume's corner, and texels a block.
    pub origin: Vec4,
    /// Its size in blocks.
    pub size: Vec4,
}

impl MaterialExtension for CreatureSkin {
    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/creature_skin.wgsl".into()
    }
}

/// A species' body: its mesh, weighted to one joint per part, and its shape for painting.
struct SpeciesBody {
    mesh: Handle<Mesh>,
    binds: Handle<SkinnedMeshInverseBindposes>,
    shape: std::sync::Arc<Shape>,
    /// The skin's volume: its corner and size in texels.
    origin: Vec3,
    dims: [u32; 3],
}

/// A creature as the client shows it.
struct Shown {
    view: CreatureView,
    /// When the view arrived, in seconds since startup.
    told_at: f32,
    position: Vec3,
    yaw: f32,
    entity: Entity,
    /// Its body's mesh, and one joint per part, which the animation turns.
    body: Entity,
    joints: Vec<(Entity, Part)>,
    /// Its skin, which is darkened by the light and flashed red, and the skin being painted
    /// for it, until it is ready.
    material: Handle<SkinMaterial>,
    painting: Option<Task<Image>>,
    skin: Handle<Image>,
    /// Stride phase, in cycles.
    stride: f32,
    /// Seconds until it may make an idle noise or call again.
    quiet: f32,
    flash: f32,
    /// Seconds since it died, if it did; the side it falls to (away from the eye), and
    /// what its fall did, for the gate: how far it strayed in heights, whether the eye was
    /// ever inside it.
    dying: Option<f32>,
    fall_side: f32,
    death_strayed: f32,
    death_around_eye: bool,
    /// While it falls, what is drawn: the furthest any joint of its body went from the
    /// middle of where it stood, in its heights, and the most any joint's scale strayed from
    /// standing (a stretched mesh shows there first).
    death_spread: f32,
    death_scale: f32,
    death_samples: u32,
    /// Whether we have struck at it, so the target readout shows its health.
    struck: bool,
    /// Smoothed head pitch and turn, and body crouch.
    nod: f32,
    look: f32,
    crouch: f32,
    /// How far a flier's wings are spread, 0 (folded, on the ground) ..= 1 (in flight).
    spread: f32,
    /// Blocks one that died in the air has fallen so far.
    fell: f32,
    /// Hearts over it, made when first needed: trust filling while it is tamed, and all
    /// red while a kept one is ready to breed.
    hearts: Vec<Entity>,
}


/// Every creature the server has told us about.
#[derive(Resource, Default)]
pub struct Creatures {
    shown: FxHashMap<u32, Shown>,
    /// Views and departures received since they were last applied.
    inbox: Vec<CreatureView>,
    left: Vec<CreatureGone>,
    /// Creatures that came into view but wait for a later frame to be built (`SPAWNS`).
    waiting: Vec<CreatureView>,
    /// Creatures our blows landed on since the last frame, to cry out.
    cries: Vec<u32>,
    /// How each creature seen to die fell, for the gate's check.
    pub falls: Vec<Fall>,
    /// Each species' body, made when the first of it is seen.
    bodies: FxHashMap<CreatureId, SpeciesBody>,
    /// The skin a body wears until its own is painted (it is hidden meanwhile).
    blank: Option<Handle<Image>>,
    /// A shaded cube for hearts, and their colours: full and empty.
    heart: Option<(Handle<Mesh>, Handle<StandardMaterial>, Handle<StandardMaterial>)>,
    /// Creatures seen to die since joining.
    pub deaths: u32,
    /// Blows of ours the server took since joining, how many landed, and the most one did.
    pub blows: u32,
    pub landed: u32,
    pub hardest: u16,
    /// The creature we last struck at, and when, in seconds since startup.
    fighting: Option<(u32, f32)>,
    /// Our own player's id, to know our kept creatures.
    pub me: Option<voxmar_protocol::PlayerId>,
}

/// How a dead creature's body fell: how far from where it stood, in its heights, and
/// whether the eye was ever inside it (it is hidden then, never drawn around the camera).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fall {
    pub strayed: f32,
    pub around_eye: bool,
    /// As drawn: the furthest a joint went from where it stood, in heights, and the most a
    /// joint's scale strayed from standing.
    pub spread: f32,
    pub scale: f32,
    /// Joint samples taken while it tipped over, and how many joints it has.
    pub samples: u32,
    pub joints: u32,
}

/// A creature as the target readout shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sighting {
    pub species: CreatureId,
    pub health: u16,
    /// Whether we have struck at it: its health is shown only then, as in RuneScape.
    pub struck: bool,
    /// Who keeps it, if anyone: ourselves, someone else (by id, while they are here), or
    /// someone away.
    pub keeper: Option<Keeper>,
    pub young: bool,
    /// Fed and looking for a mate.
    pub ready: bool,
}

/// Who keeps a kept creature, as the target readout tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keeper {
    Us,
    Other(voxmar_protocol::PlayerId),
    Away,
}

impl Creatures {
    /// Takes what the server sent.
    pub fn receive(&mut self, changed: Vec<CreatureView>, gone: Vec<CreatureGone>) {
        self.inbox.extend(changed);
        self.left.extend(gone);
    }

    /// The server took a blow of ours at `id`: it landed for `damage`, or missed.
    pub fn struck(&mut self, id: u32, damage: u16, _killed: bool, now: f32) {
        self.blows += 1;
        self.fighting = Some((id, now));
        if let Some(shown) = self.shown.get_mut(&id) {
            shown.struck = true;
        }
        if damage > 0 {
            self.landed += 1;
            self.hardest = self.hardest.max(damage);
            if let Some(shown) = self.shown.get_mut(&id) {
                shown.flash = FLASH;
            }
            // Every blow that lands is a cry, the killing one too: a full charge can kill
            // a small creature outright, with no health drop seen before it died.
            self.cries.push(id);
        }
    }

    /// Creature `id` as the target readout shows it; one dying still is, at no health.
    /// Every living creature shown, by id: its species and where its feet are.
    pub fn living(&self) -> impl Iterator<Item = (u32, CreatureId, Vec3)> + '_ {
        self.shown.iter().filter(|(_, shown)| shown.dying.is_none()).map(|(&id, shown)| (id, shown.view.species, shown.position))
    }

    pub fn sighting(&self, id: u32) -> Option<Sighting> {
        let shown = self.shown.get(&id)?;
        let health = if shown.dying.is_some() { 0 } else { shown.view.health };
        let view = &shown.view;
        let keeper = view.kept.then(|| match view.owner {
            Some(owner) if Some(owner) == self.me => Keeper::Us,
            Some(owner) => Keeper::Other(owner),
            None => Keeper::Away,
        });
        Some(Sighting { species: view.species, health, struck: shown.struck, keeper, young: view.young, ready: view.ready })
    }

    /// The creature we struck at in the last `within` seconds before `now`, while it is
    /// still shown (alive, or falling).
    pub fn fighting(&self, now: f32, within: f32) -> Option<u32> {
        self.fighting.filter(|&(id, at)| now - at < within && self.shown.contains_key(&id)).map(|(id, _)| id)
    }

    /// How many creatures are shown, alive.
    pub fn alive(&self) -> usize {
        self.shown.values().filter(|s| s.dying.is_none()).count()
    }

    /// The creatures shown alive, by id: species and where they stand.
    pub fn views(&self) -> impl Iterator<Item = &CreatureView> {
        self.shown.values().filter(|s| s.dying.is_none()).map(|s| &s.view)
    }

    /// The gate checks both eyes in the actual painted skin worn by a summoned shambler.
    /// None means its body or asynchronous skin has not reached the client yet.
    pub(crate) fn front_eyes(&self, launch: &Launch, images: &Assets<Image>) -> Option<Result<String, String>> {
        let species = launch.content.creatures.find("base:shambler")?;
        let shown = self.shown.values().find(|s| s.view.species == species && s.dying.is_none())?;
        if shown.painting.is_some() {
            return None;
        }
        let image = images.get(&shown.skin)?;
        let pixels = image.data.as_ref()?;
        let body = self.bodies.get(&species)?;
        let head = shown.joints.iter().map(|(_, p)| p).find(|p| p.role == creature::Role::Head)?;
        let def = &launch.content.creatures.get(species).def.body;
        let Some(radius) = def.skin.eye.map(|r| r * def.scale) else {
            return Some(Err("the front-eye fixture has no skin.eye radius".into()));
        };
        let [w, h, d] = body.dims.map(|n| n as usize);
        let mut eyes = [[0usize; 3]; 2];
        for (side, counts) in eyes.iter_mut().enumerate() {
            let sign = if side == 0 { -1.0 } else { 1.0 };
            let eye = Vec3::new(sign * head.size.x * 0.24, head.size.y * 0.1, -head.size.z * 0.5);
            for z in 0..d {
                for y in 0..h {
                    for x in 0..w {
                        let at = body.origin + (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / skin::TEXELS;
                        let local = head.rest.inverse() * (at - head.centre);
                        let delta = (local - eye).abs();
                        if delta.x > radius || delta.y > radius || delta.z > 1.0 / skin::TEXELS {
                            continue;
                        }
                        let at = ((z * h + y) * w + x) * 4;
                        let c = &pixels[at..at + 4];
                        if c[3] == 0 {
                            continue;
                        }
                        counts[0] += usize::from(c[..3].iter().all(|&v| v > 170) && c[0].abs_diff(c[2]) < 8);
                        counts[1] += usize::from(c[1] > 140 && c[1] > c[0] && c[2] < 130);
                        counts[2] += usize::from(c[..3].iter().all(|&v| v < 30));
                    }
                }
            }
        }
        let detail = format!("sclera/catchlight, iris, pupil texels on each front eye: {eyes:?}");
        Some(if eyes.iter().all(|counts| counts.iter().all(|&n| n > 0)) { Ok(detail) } else { Err(detail) })
    }

    /// The box a creature is drawn in, by its species' size and its own scale.
    /// The living creature along a ray from `eye`, within `reach` and nearer than `block`.
    pub fn pointed(&self, launch: &Launch, eye: Vec3, look: Vec3, reach: f32, block: f32) -> Option<u32> {
        self.shown
            .keys()
            .filter_map(|&id| Some((id, ray_box(eye, look, &self.aabb(launch, id)?)?)))
            .filter(|&(_, d)| d <= reach && d < block)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// Every living creature's body, for the player to bump against (`player::movement`).
    pub fn bodies<'a>(&'a self, launch: &'a Launch) -> impl Iterator<Item = Aabb> + 'a {
        self.shown.keys().filter_map(move |&id| self.aabb(launch, id))
    }

    /// The middle of creature `id`'s body while it is shown, alive or falling, and whether
    /// it is falling: for marks that follow a body to its end (the killing blow's hitsplat).
    pub fn centre(&self, launch: &Launch, id: u32) -> Option<(Vec3, bool)> {
        let shown = self.shown.get(&id)?;
        let height = launch.content.creatures.get(shown.view.species).def.body.size.1 * shown.view.scale;
        Some((shown.position + Vec3::Y * height * 0.5, shown.dying.is_some()))
    }

    /// The creature nearest along the look ray within `within` blocks of the eye, and how far.
    pub fn looked_at(&self, launch: &Launch, eye: Vec3, look: Vec3, within: f32) -> Option<(u32, f32)> {
        self.shown
            .keys()
            .filter_map(|&id| Some((id, ray_box(eye, look, &self.aabb(launch, id)?)?)))
            .filter(|&(_, distance)| distance <= within)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    fn aabb(&self, launch: &Launch, id: u32) -> Option<Aabb> {
        let shown = self.shown.get(&id).filter(|s| s.dying.is_none())?;
        let size = launch.content.creatures.get(shown.view.species).def.body.size;
        Some(Aabb::standing(shown.position, size.0 * shown.view.scale, size.1 * shown.view.scale))
    }
}

/// How bright each face is drawn, as the sun lights a box from above: top brightest, then
/// the sides, the bottom darkest. In [`FACES`] order.
const SHADES: [f32; 6] = [1.0, 0.5, 0.8, 0.7, 0.75, 0.85];

/// A unit cube centred on the origin with its faces shaded and its texture coordinates in
/// `rects` of a `layout`-sized texture (each face its whole patch), the corners laid out
/// as the skin painter lays its texels ([`FACES`]).
fn part_mesh(rects: &[skin::Rect; 6], layout: (u32, u32)) -> Mesh {
    let (mut positions, mut normals, mut colours, mut uvs, mut indices) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for ((face, rect), shade) in FACES.iter().zip(rects).zip(SHADES) {
        let base = positions.len() as u32;
        // Corners counter-clockwise seen from outside: (u, v) of (0,1), (1,1), (1,0), (0,0).
        for (u, v) in [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
            let corner = face.normal * 0.5 + face.u * (u - 0.5) + face.v * (v - 0.5);
            positions.push(corner.to_array());
            normals.push(face.normal.to_array());
            colours.push([shade, shade, shade, 1.0]);
            uvs.push([(rect.x as f32 + u * rect.w as f32) / layout.0 as f32, (rect.y as f32 + v * rect.h as f32) / layout.1 as f32]);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// A whole-texture patch on every face, for hearts and warming.
fn whole(size: (u32, u32)) -> [skin::Rect; 6] {
    [skin::Rect { x: 0, y: 0, w: size.0, h: size.1 }; 6]
}

/// A species' body as a skinned mesh: rest positions (in the vertex colour, for the skin),
/// normals, and each vertex's four joints and weights.
fn body_mesh(body: &BodyMesh) -> Mesh {
    let rest: Vec<[f32; 4]> = body.positions.iter().map(|p| [p[0], p[1], p[2], 1.0]).collect();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, body.positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, body.normals.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, rest)
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(body.joints.clone()))
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, body.weights.clone())
        .with_inserted_indices(Indices::U32(body.indices.clone()))
}

/// Builds a species' body from its parts.
fn species_body(model: &creature::Model, parts: &[Part], meshes: &mut Assets<Mesh>, binds: &mut Assets<SkinnedMeshInverseBindposes>) -> SpeciesBody {
    let anatomy = creature::anatomy::of(model, parts);
    let body = volume::build(parts, anatomy.as_ref());
    let shape = std::sync::Arc::new(Shape::new(parts, anatomy.as_ref()));
    // Each joint sits at its part's pivot; at rest it moves nothing.
    let inverse: Vec<Mat4> = parts.iter().map(|p| Mat4::from_translation(-p.pivot)).collect();
    let texel = 1.0 / skin::TEXELS;
    let origin = ((body.min / texel).floor() - 1.0) * texel;
    let dims = (((body.max - origin) / texel).ceil() + 1.0).as_uvec3().to_array();
    SpeciesBody { mesh: meshes.add(body_mesh(&body)), binds: binds.add(SkinnedMeshInverseBindposes::from(inverse)), shape, origin, dims }
}

/// Paints an individual's skin through its species' body volume, off the main thread, so
/// a herd coming into view never holds a frame: the body shows once its skin is ready.
fn paint_skin(def: creature::BodyDef, parts: Vec<Part>, body: &SpeciesBody, seed: u32, inspect: bool) -> Task<Image> {
    let (shape, origin, dims) = (body.shape.clone(), body.origin, body.dims);
    AsyncComputeTaskPool::get().spawn(async move {
        let shell = shape.cell * 0.75 + 1.5 / skin::TEXELS;
        // The part each point belongs to, at the body's own surface distance.
        let pixels = skin::paint_volume(&def, &parts, seed, origin, dims, shell, |p| shape.nearest(p).map(|(i, _)| (i, shape.distance(p))));
        let [w, h, d] = dims;
        let extent = Extent3d { width: w, height: h, depth_or_array_layers: d };
        let usage = if inspect { RenderAssetUsages::all() } else { RenderAssetUsages::RENDER_WORLD };
        let mut image = Image::new(extent, TextureDimension::D3, pixels, TextureFormat::Rgba8UnormSrgb, usage);
        image.sampler = nearest();
        image
    })
}

/// What is drawn of bodies: whether each is shown, and where the last frame put its joints.
#[derive(bevy::ecs::system::SystemParam)]
struct Drawn<'w, 's> {
    visible: Query<'w, 's, &'static mut Visibility>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    inspect: Option<Res<'w, InspectEyes>>,
}

/// The light creatures are lit by and the air they are fogged by, as the terrain's.
#[derive(bevy::ecs::system::SystemParam)]
struct Air<'w> {
    light: Res<'w, crate::render::SceneLight>,
    atmosphere: Res<'w, crate::render::atmosphere::SharedAtmosphere>,
}

/// The near plane's reach: a box this much closer than the eye is already cut open.
const NEAR_PLANE: f32 = 0.1;

/// Whether the eye is inside a living body: inside one of its part boxes (grown by the near
/// plane), in the creature's own frame. A fallen body lies on its side, so a dying one counts
/// within its height of where it stood.
fn eye_inside(parts: &[Part], eye: Vec3, position: Vec3, yaw: f32, scale: f32, height: f32, dying: bool) -> bool {
    let from = eye - position;
    if dying {
        return from.with_y(0.0).length() < height + NEAR_PLANE && (-0.2..height + 0.2).contains(&from.y);
    }
    let local = Quat::from_rotation_y(yaw).inverse() * from / scale.max(1e-3);
    let grow = NEAR_PLANE / scale.max(1e-3);
    parts.iter().any(|part| {
        let inside = part.rest.inverse() * (local - part.centre);
        let half = part.size * 0.5 + Vec3::splat(grow);
        inside.abs().cmple(half).all()
    })
}

/// A sound a creature makes now.
fn sound(kind: Sound, id: u32, view: &CreatureView, position: Vec3, speed: f32) -> CreatureSound {
    CreatureSound { kind, id, species: view.species, position, speed, scale: view.scale, young: view.young }
}

/// Applies what the server sent, then moves and animates every creature, and handles
/// aiming at and striking them.
#[allow(clippy::too_many_arguments)]
fn show(
    mut commands: Commands,
    time: Res<Time>,
    launch: Res<Launch>,
    world: Res<ClientWorld>,
    air: Air,
    player: Res<LocalPlayer>,
    mut creatures: ResMut<Creatures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut skins: ResMut<Assets<SkinMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut transforms: Query<&mut Transform>,
    mut looks: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut drawn: Drawn,
    mut sounds: ResMut<CreatureSounds>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    // Their pipelines (a skinned box, and a heart's clipped one) compile as the world loads
    // (`render::warm`), not when the first creature comes into view.
    // Nothing is drawn until the renderer has made the air they are fogged by.
    let Some(atmosphere) = air.atmosphere.get() else { return };
    let scene_light = &air.light;
    let (heart_mesh, heart_full, heart_empty) = creatures
        .heart
        .get_or_insert_with(|| {
            let image = images.add(heart_image());
            let mesh = meshes.add(part_mesh(&whole((1, 1)), (1, 1)));
            // A body's pipeline: a skinned box with a one-texel skin.
            let cube = BodyMesh {
                positions: FACES.iter().flat_map(|f| [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(a, b)| (f.normal * 0.5 + f.u * a + f.v * b).to_array())).collect(),
                normals: FACES.iter().flat_map(|f| [f.normal.to_array(); 4]).collect(),
                joints: vec![[0; 4]; 24],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 24],
                indices: (0..6u32).flat_map(|f| [0, 2, 1, 0, 3, 2].map(|i| f * 4 + i)).collect(),
                ..default()
            };
            let pixel = Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D3, vec![255; 4], TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
            let body = skins.add(SkinMaterial { base: StandardMaterial { unlit: true, ..default() }, extension: CreatureSkin { skin: images.add(pixel), frame: SkinFrame { origin: Vec4::new(-1.0, -1.0, -1.0, skin::TEXELS), size: Vec4::splat(2.0) }, atmosphere: atmosphere.clone() } });
            let warm_binds = binds.add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY]));
            crate::render::warm::warm_skinned(&mut commands, meshes.add(body_mesh(&cube)), body, warm_binds, DespawnOnExit(Screen::InGame));
            let heart = |colour: Color| StandardMaterial { base_color: colour, base_color_texture: Some(image.clone()), alpha_mode: AlphaMode::Mask(0.5), unlit: true, ..default() };
            let full = materials.add(heart(Color::srgb(0.95, 0.16, 0.24)));
            let empty = materials.add(heart(Color::srgb(0.3, 0.27, 0.3)));
            // Hearts cast no shadow, but a clipped material's depth pipeline is warmed too, in
            // case any pass draws them.
            crate::render::warm::warm(&mut commands, mesh.clone(), full.clone(), true, DespawnOnExit(Screen::InGame));
            (mesh, full, empty)
        })
        .clone();
    let registry = &launch.content.creatures;

    let creatures = &mut *creatures;
    for gone in std::mem::take(&mut creatures.left) {
        creatures.waiting.retain(|view| view.id != gone.id);
        match creatures.shown.get_mut(&gone.id) {
            Some(shown) if gone.died => {
                shown.dying = Some(0.0);
                sounds.push(sound(Sound::Death, gone.id, &shown.view, shown.position, 0.0));
                creatures.deaths += 1;
            }
            Some(shown) => {
                despawn(&mut commands, shown);
                if let Some(shown) = creatures.shown.remove(&gone.id) {
                    if shown.painting.is_none() {
                images.remove(&shown.skin);
            }
                    skins.remove(&shown.material);
                }
            }
            None => {}
        }
    }
    let cried = std::mem::take(&mut creatures.cries);
    for &id in &cried {
        if let Some(shown) = creatures.shown.get(&id) {
            sounds.push(sound(Sound::Hurt, id, &shown.view, shown.position, 0.0));
        }
    }
    let mut built = 0;
    let views: Vec<CreatureView> = std::mem::take(&mut creatures.waiting).into_iter().chain(std::mem::take(&mut creatures.inbox)).collect();
    for view in views {
        if let Some(shown) = creatures.shown.get_mut(&view.id) {
            // Hurt by someone else's blow (ours cry as the server answers them, below).
            if view.health < shown.view.health && !cried.contains(&view.id) {
                shown.flash = FLASH;
                sounds.push(sound(Sound::Hurt, view.id, &view, shown.position, 0.0));
            }
            // What it does now, heard as it starts.
            if view.pose != shown.view.pose {
                let kind = match view.pose {
                    Pose::Grazing => Some(Sound::Graze),
                    Pose::Alert => Some(Sound::Alert),
                    Pose::Resting => Some(Sound::Rest),
                    Pose::Attacking => Some(Sound::Attack),
                    Pose::Idle | Pose::Crouched => None,
                };
                if let Some(kind) = kind {
                    sounds.push(sound(kind, view.id, &view, shown.position, 0.0));
                }
            }
            if view.ready && !shown.view.ready {
                sounds.push(sound(Sound::Ready, view.id, &view, shown.position, 0.0));
            }
            shown.view = view;
            shown.told_at = now;
            continue;
        }
        if view.species.0 as usize >= registry.len() {
            continue;
        }
        // A herd coming into view at once is built a few a frame: painting a skin and
        // spawning a model took a frame 4.5 ms longer for a handful.
        if built == SPAWNS || creatures.waiting.iter().any(|waiting| waiting.id == view.id) {
            creatures.waiting.push(view);
            continue;
        }
        built += 1;
        let species = registry.get(view.species);
        let body = creatures.bodies.entry(view.species).or_insert_with(|| species_body(&species.def.body.model, &species.parts, &mut meshes, &mut binds));
        // Painted as its genes show: its coat colours and markings.
        let genes = &species.def.genes;
        let traits = voxmar_engine::genes::express(genes, &voxmar_engine::genes::unpack(genes, &view.genes));
        let painting = Some(paint_skin(species.def.body.with(&traits), species.parts.clone(), body, view.seed, drawn.inspect.is_some() && species.def.name == "base:shambler"));
        let skin = creatures.blank.get_or_insert_with(|| images.add(blank_skin())).clone();
        let size = Vec3::from_array(body.dims.map(|n| n as f32)) / skin::TEXELS;
        let frame = SkinFrame { origin: body.origin.extend(skin::TEXELS), size: size.extend(0.0) };
        let material = skins.add(SkinMaterial { base: StandardMaterial { unlit: true, ..default() }, extension: CreatureSkin { skin: skin.clone(), frame, atmosphere: atmosphere.clone() } });
        let entity = commands.spawn((Transform::from_translation(view.position), Visibility::default(), DespawnOnExit(Screen::InGame))).id();
        let joints: Vec<(Entity, Part)> = species.parts.iter().map(|&part| (commands.spawn((Transform::from_translation(part.pivot), ChildOf(entity))).id(), part)).collect();
        let skinned = SkinnedMesh { inverse_bindposes: body.binds.clone(), joints: joints.iter().map(|(e, _)| *e).collect() };
        let body = commands.spawn((Mesh3d(body.mesh.clone()), MeshMaterial3d(material.clone()), skinned, Transform::default(), Visibility::default(), ChildOf(entity))).id();
        creatures.shown.insert(
            view.id,
            Shown {
                view,
                told_at: now,
                position: view.position,
                yaw: view.yaw,
                entity,
                body,
                joints,
                material,
                painting,
                skin,
                stride: 0.0,
                quiet: 2.0 + creature::unit_from(creature::mix(view.seed as u64, 77)) * 8.0,
                flash: 0.0,
                dying: None,
                fall_side: 0.0,
                death_strayed: 0.0,
                death_around_eye: false,
                death_spread: 0.0,
                death_scale: 0.0,
                death_samples: 0,
                struck: false,
                nod: 0.0,
                look: 0.0,
                crouch: 0.0,
                spread: 0.0,
                fell: 0.0,
                hearts: Vec::new(),
            },
        );
    }

    let eye = player.eye();
    let me = creatures.me;
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
        let height = def.body.size.1 * scale;
        let before_step = (shown.stride * 2.0).floor();
        shown.stride += speed * dt / (def.body.size.0.max(0.3) * 1.6 * scale);
        // A footfall each half stride, while it walks on something.
        if (shown.stride * 2.0).floor() != before_step && speed > 0.3 && shown.dying.is_none() && def.movement.style != Style::Drifter {
            sounds.push(sound(Sound::Step, id, &shown.view, shown.position, speed));
        }
        // Now and then, at its own pace, a contented noise, or a call to its kind.
        shown.quiet -= dt;
        if shown.quiet <= 0.0 && shown.dying.is_none() {
            let roll = creature::unit_from(creature::mix(shown.view.seed as u64, (now * 10.0) as u64));
            let kind = if roll < 0.25 { Sound::Call } else { Sound::Idle };
            sounds.push(sound(kind, id, &shown.view, shown.position, speed));
            shown.quiet = 6.0 + roll * 18.0;
        }
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
        // Watching the player who comes near, unless grazing or running.
        let to_eye = eye - shown.position;
        let watching = to_eye.length() < WATCHES_WITHIN && shown.view.pose != Pose::Grazing && speed < def.movement.walk * 1.2 && shown.dying.is_none();
        let look = if watching {
            let facing = (-to_eye.x).atan2(-to_eye.z);
            ((facing - shown.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).clamp(-WATCH_TURN, WATCH_TURN)
        } else {
            0.0
        };
        shown.look += (look - shown.look) * (4.0 * dt).min(1.0);

        // A flier spreads its wings off the ground, and falls when it dies in the air.
        let flier = def.movement.style == Style::Flier;
        let aloft = flier && !world.is_solid((shown.position - Vec3::Y * 0.1).floor().as_ivec3());
        shown.spread += (f32::from(u8::from(aloft && shown.dying.is_none())) - shown.spread) * (8.0 * dt).min(1.0);
        let falling = aloft && shown.dying.is_some() && shown.fell < FALLS_MOST;
        if falling {
            shown.fell += FALLS * dt;
            shown.position.y -= FALLS * dt;
            shown.view.position = shown.position;
            shown.view.velocity = Vec3::ZERO;
        }
        let moving = (speed / (def.movement.walk.max(0.5) * 1.5)).min(1.0) * (1.0 - shown.spread);
        let running = speed > def.movement.walk * 1.6;
        // Hoppers hop, and anything running bounds: pairs together. Otherwise diagonal pairs.
        let paired = def.movement.style == Style::Hopper || (running && def.movement.style != Style::Creeper);
        let step = shown.stride * std::f32::consts::TAU;
        let bob = if paired { (step.sin().abs() * 0.12 - 0.02).max(0.0) * moving } else { step.sin().abs() * 0.025 * moving };
        let bob = match def.movement.style {
            Style::Drifter => (now * 1.7 + id as f32).sin() * 0.06,
            // Small birds rise and dip as they fly, a burst of wingbeats and a glide.
            Style::Flier => bob * (1.0 - shown.spread) + shown.spread * flight_bound(now, id).0 * UNDULATE / height.max(0.05),
            _ => bob,
        };

        let mut root = Transform::from_translation(shown.position + Vec3::Y * height * (bob - shown.crouch))
            .with_rotation(Quat::from_rotation_y(shown.yaw))
            .with_scale(Vec3::splat(scale));
        if let Some(dying) = shown.dying.as_mut() {
            // One that died in the air rolls over as it falls, and sinks away only once it is
            // down, however high it flew.
            let death = &def.death;
            *dying = if falling { (*dying + dt).min(death.seconds * 0.5) } else { *dying + dt };
            // It falls away from the eye: rolled toward the eye at melee range, its back
            // came through the camera and showed the inside of the body.
            if shown.fall_side == 0.0 {
                let local = Quat::from_rotation_y(shown.yaw).inverse() * (eye - shown.position);
                shown.fall_side = if local.x >= 0.0 { 1.0 } else { -1.0 };
            }
            let t = (*dying / death.seconds.max(0.05)).min(1.0);
            root.rotation *= Quat::from_rotation_z(shown.fall_side * (t * 2.0).min(1.0) * death.roll.to_radians());
            // How far it strays from where it stood as it falls, in heights, for the gate
            // (before it sinks away, which is meant to take it out of sight).
            let middle = root.translation + root.rotation * Vec3::Y * height * 0.5;
            let strayed = middle.distance(shown.position + Vec3::Y * height * 0.5) / height.max(0.1);
            root.translation.y -= (t * 2.0 - 1.0).max(0.0) * death.sink * height;
            shown.death_strayed = shown.death_strayed.max(strayed);
            if t >= 1.0 {
                finished.push(id);
            }
        }
        if let Ok(mut transform) = transforms.get_mut(shown.entity) {
            *transform = root;
        }
        // What the last frame drew of a falling body, joint by joint, for the gate: while it
        // tips over, before it starts to sink away.
        let sinking = shown.dying.is_some_and(|d| d >= def.death.seconds * 0.5);
        if shown.dying.is_some() && !sinking {
            let middle = shown.position + Vec3::Y * height * 0.5;
            for (joint, _) in &shown.joints {
                if let Ok(global) = drawn.globals.get(*joint) {
                    let (s, _, t) = global.to_scale_rotation_translation();
                    shown.death_spread = shown.death_spread.max(t.distance(middle) / height.max(0.1));
                    shown.death_scale = shown.death_scale.max((s / scale.max(1e-3) - Vec3::ONE).abs().max_element());
                    shown.death_samples += 1;
                }
            }
        }
        let far = shown.position.distance(eye) > DRAWN_WITHIN;
        // Hearts: trust filling while a wild one is being tamed, all full while a kept one
        // is ready to breed, bobbing over it and turned to face the eye.
        let filled = if shown.view.ready { HEARTS } else { (shown.view.trust as usize * HEARTS).div_ceil(100) };
        let showing = (shown.view.ready || (!shown.view.kept && shown.view.trust > 0)) && shown.dying.is_none() && !far;
        if showing && shown.hearts.is_empty() {
            shown.hearts = (0..HEARTS)
                .map(|_| commands.spawn((Mesh3d(heart_mesh.clone()), MeshMaterial3d(heart_empty.clone()), Transform::default(), NotShadowCaster, DespawnOnExit(Screen::InGame))).id())
                .collect();
        }
        let above = shown.position + Vec3::Y * (height + 0.3 + (now * 3.0 + id as f32).sin() * 0.04);
        let toward = (eye - above).with_y(0.0).normalize_or(Vec3::Z);
        let facing = Quat::from_rotation_y(toward.x.atan2(toward.z));
        let across = facing * Vec3::X;
        for (i, heart) in shown.hearts.iter().enumerate() {
            if let Ok(mut transform) = transforms.get_mut(*heart) {
                let offset = (i as f32 - (HEARTS as f32 - 1.0) / 2.0) * 0.27;
                let size = if showing { Vec3::new(0.25, 0.2, 0.01) } else { Vec3::ZERO };
                *transform = Transform::from_translation(above + across * offset).with_rotation(facing).with_scale(size);
            }
            let colour = if i < filled { &heart_full } else { &heart_empty };
            if let Ok(mut look) = looks.get_mut(*heart)
                && look.0 != *colour
            {
                look.0 = colour.clone();
            }
        }

        // A kept creature wags faster when its keeper is near.
        let keeper_near = shown.view.kept && shown.view.owner.is_some() && shown.view.owner == me && to_eye.length() < 6.0;
        let breath = 1.0 + (now * 2.2 + id as f32 * 0.7).sin() * 0.012 * (1.0 - moving);
        let twitch = |side: f32| {
            // Now and then, one ear flicks back.
            let slot = (now * 0.8 + id as f32 * 0.37 + side * 0.21).floor();
            let chance = creature::unit_from(creature::mix(id as u64 ^ slot as i64 as u64, side.to_bits() as u64));
            if chance < 0.18 { ((now * 0.8 + id as f32 * 0.37 + side * 0.21).fract() * std::f32::consts::TAU).sin().max(0.0) * 0.6 } else { 0.0 }
        };
        // Its skin, once painted.
        if let Some(task) = shown.painting.as_mut()
            && let Some(image) = block_on(future::poll_once(task))
        {
            shown.painting = None;
            shown.skin = images.add(image);
            if let Some(mut material) = skins.get_mut(&shown.material) {
                material.extension.skin = shown.skin.clone();
            }
        }
        // Never drawn around the eye: inside a body only its inside shows, so a body the eye
        // is inside is hidden.
        let around_eye = eye_inside(&registry.get(shown.view.species).parts, eye, shown.position, shown.yaw, scale, height, shown.dying.is_some());
        if around_eye && shown.dying.is_some() {
            shown.death_around_eye = true;
        }
        if let Ok(mut shows) = drawn.visible.get_mut(shown.body) {
            let want = if far || around_eye || shown.painting.is_some() { Visibility::Hidden } else { Visibility::Inherited };
            if *shows != want {
                *shows = want;
            }
        }
        for (child, part) in &shown.joints {
            let (rotation, stretch) = match part.swing {
                Swing::Leg { phase, hind } => {
                    let phase = if paired { if hind { 0.0 } else { 0.5 } } else { phase };
                    let reach = if paired { 0.9 } else { 0.7 };
                    (Quat::from_rotation_x(((shown.stride + phase) * std::f32::consts::TAU).sin() * reach * moving), Vec3::ONE)
                }
                Swing::Scuttle { phase } => (Quat::from_rotation_y(((shown.stride * 1.5 + phase) * std::f32::consts::TAU).sin() * 0.45 * moving), Vec3::ONE),
                Swing::Head => (Quat::from_rotation_y(shown.look) * Quat::from_rotation_x(-shown.nod), Vec3::ONE),
                Swing::Ear { side } => (Quat::from_rotation_x(twitch(side)) * Quat::from_rotation_y(shown.look), Vec3::ONE),
                Swing::Tail => {
                    let (rate, sway) = if keeper_near { (11.0, 0.45) } else { (2.5, 0.3) };
                    (Quat::from_rotation_y((now * rate + id as f32).sin() * sway + speed.min(4.0) * 0.05), Vec3::ONE)
                }
                Swing::Wing { side } if flier => {
                    // Spread out to the sides and lengthened in flight, beating in bursts.
                    let (_, beating) = flight_bound(now, id);
                    let beat = (now * WINGBEATS).sin() * 0.75 * beating;
                    let angle = shown.spread * (WING_SPREAD + beat);
                    (Quat::from_rotation_z(side * angle), Vec3::new(1.0, 1.0 + WING_REACH * shown.spread, 1.0))
                }
                Swing::Wing { side } => {
                    let beat = if running || speed > def.movement.walk * 1.2 { (now * 18.0).sin() * 0.6 } else { 0.0 };
                    (Quat::from_rotation_z(side * beat), Vec3::ONE)
                }
                Swing::Orbit { phase } => (Quat::from_rotation_y((now * 1.3 + phase) * std::f32::consts::TAU), Vec3::ONE),
                Swing::Still if part.role == creature::Role::Torso => (Quat::IDENTITY, Vec3::new(1.0, breath, 1.0)),
                Swing::Still => (Quat::IDENTITY, Vec3::ONE),
            };
            // The joint turns its part about the pivot (its rest is folded into the mesh).
            if let Ok(mut transform) = transforms.get_mut(*child) {
                *transform = Transform { translation: part.pivot, rotation, scale: stretch };
            }
        }
        // Lit exactly as the terrain around it is (`SceneLight`: at night as dark as the
        // ground, which a separate daylight curve left far brighter), a glowing creature by
        // itself; red when struck.
        let cell = (shown.position + Vec3::Y * 0.5).floor().as_ivec3();
        let light = if def.body.glows {
            LinearRgba::WHITE
        } else {
            scene_light.at(sky_light(&world, cell), block_light(&world, cell))
        };
        // In steps too fine to see, and written only when it changes: a changed material
        // is uploaded again and every part drawn with it specialised again for the camera
        // and each shadow cascade: writing it every frame took a herd of 40 from 4 to 8 ms
        // a frame on an NVIDIA L4.
        let step = |c: f32| (c * LIGHT_STEPS).round() / LIGHT_STEPS;
        let red = shown.flash / FLASH;
        let lit = LinearRgba::rgb(step(light.red), step(light.green), step(light.blue));
        let colour = Color::LinearRgba(lit.mix(&LinearRgba::rgb(0.9, 0.08, 0.05), red * 0.7));
        if skins.get(&shown.material).is_some_and(|material| material.base.base_color != colour)
            && let Some(mut material) = skins.get_mut(&shown.material)
        {
            material.base.base_color = colour;
        }
    }
    for id in finished {
        if let Some(shown) = creatures.shown.remove(&id) {
            creatures.falls.push(Fall { strayed: shown.death_strayed, around_eye: shown.death_around_eye, spread: shown.death_spread, scale: shown.death_scale, samples: shown.death_samples, joints: shown.joints.len() as u32 });
            despawn(&mut commands, &shown);
            if shown.painting.is_none() {
                images.remove(&shown.skin);
            }
            skins.remove(&shown.material);
        }
    }
}

/// Finds the creature under the crosshair, nearer than any block, and strikes it on a
/// left click. Runs between finding the block target and breaking, so aiming at a
/// creature never mines the ground behind it.
#[allow(clippy::too_many_arguments)]
pub fn aim(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiState>,
    launch: Res<Launch>,
    net: Res<Net>,
    actions: Res<Actions>,
    player: Res<LocalPlayer>,
    creatures: Res<Creatures>,
    mut target: ResMut<Target>,
    mut fighting: ResMut<crate::fight::Fighting>,
) {
    // What the right button did this frame, if anything, is said afresh: here, then in
    // `interact::break_and_place`; `fight::guard` raises no guard if it did something.
    target.used_right = false;
    let eye = player.eye();
    let look = player.look_direction().normalize_or_zero();
    let block = target.hit.map_or(f32::MAX, |hit| hit.distance);
    // What a blow would be struck with: its reach and rhythm, and whether it guards.
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let combat = &launch.content.combat;
    let weapon = combat.wielded(held, |skill| launch.content.skills.level(player.skills.xp(skill)));
    // Aimed at as far as it can be touched (tended, looked at) or struck, whichever is
    // further; struck only within the weapon's reach.
    let nearest = creatures
        .shown
        .keys()
        .filter_map(|&id| Some((id, ray_box(eye, look, &creatures.aabb(&launch, id)?)?)))
        .filter(|&(_, d)| d <= weapon.reach.max(combat.feel.touch) && d < block)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    target.creature = nearest.map(|(id, _)| id);
    let in_reach = nearest.is_some_and(|(_, d)| d <= weapon.reach);
    if target.creature.is_some() {
        target.hit = None;
    }
    // A right click tends it with what is in hand: food that tames it or readies a kept one
    // to breed, or a tool for its product (the server judges; this only asks when it may).
    // At a creature fighting the player (it struck or was struck lately, or is winding up)
    // with a weapon or shield that guards, the button guards (`fight`); otherwise it looks
    // closely at it. With Shift held, on one of your own, it asks to let it go wild (twice
    // to do it).
    if let Some(id) = target.creature
        && ui.cursor_locked
        && !player.dead
        && mouse.just_pressed(MouseButton::Right)
        && let Some(view) = creatures.shown.get(&id).map(|s| s.view)
    {
        let species = launch.content.creatures.get(view.species);
        let mine = view.kept && view.owner.is_some() && view.owner == creatures.me;
        let accepts = held.is_some_and(|item| {
            let tags = &launch.content.blocks.def(item).tags;
            let tool = species.def.products.iter().any(|p| p.tool.as_ref().is_some_and(|t| tags.contains(t)));
            // One of yours takes its food as a meal or to breed (the server judges which).
            let food = species.tame_food.contains(&item) || species.breed_food.contains(&item) || tags.iter().any(|t| t == "breeding_feed");
            (!view.kept && species.tame_food.contains(&item)) || (mine && (food || tool))
        });
        let guards = combat.guard_of(held, &player.inventory.worn, |skill| launch.content.skills.level(player.skills.xp(skill))).is_some();
        let engaged = !view.kept && (view.pose == Pose::Attacking || fighting.engaged(id, time.elapsed_secs_f64(), &combat.feel));
        let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let action = if shift && mine {
            Some("base:release")
        } else if accepts {
            Some("base:tend")
        } else if !(guards && engaged) {
            Some("base:inspect")
        } else {
            None
        };
        if let Some(action) = action.and_then(|a| actions.0.id(a)) {
            net.propose_from(&player, action, ActionTarget::Creature(id));
            target.used_right = true;
        }
    }
    // A left click swings: at once, or held for the beat (the server keeps the same
    // rhythm and says what lands).
    if let Some(id) = target.creature
        && in_reach
        && ui.cursor_locked
        && !player.dead
        && mouse.just_pressed(MouseButton::Left)
        && let Some(attack) = actions.0.id("core:attack")
        && fighting.click(time.elapsed_secs_f64(), &combat.timing(&weapon), held, &combat.feel)
    {
        net.propose_from(&player, attack, ActionTarget::Creature(id));
    }
}

/// Where a ray from `origin` along unit `direction` first enters `aabb`, if it does.
pub(crate) fn ray_box(origin: Vec3, direction: Vec3, aabb: &Aabb) -> Option<f32> {
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

    fn bear() -> Vec<Part> {
        creature::Model::Quadruped {
            body: (1.0, 0.92, 1.5),
            head: (0.54, 0.46, 0.46),
            legs: 0.55,
            ears: None,
            tail: None,
            horns: None,
            horn_shape: creature::HornShape::Curved,
            snout: None,
            neck: None,
            haunches: false,
            hump: None,
            shell: None,
            tail_droop: 0.0,
            ear_lean: 0.0,
        }
        .parts(1.0)
    }

    #[test]
    fn bodies_hide_only_with_the_eye_inside() {
        let parts = bear();
        // Beside the flank, 0.2 out: seen.
        assert!(!eye_inside(&parts, Vec3::new(0.7, 1.0, 0.0), Vec3::ZERO, 0.0, 1.0, 1.4, false));
        // Inside the torso: hidden.
        assert!(eye_inside(&parts, Vec3::new(0.2, 1.0, 0.1), Vec3::ZERO, 0.0, 1.0, 1.4, false));
        // Turned a quarter, the flank faces along z: still seen from 0.2 out.
        assert!(!eye_inside(&parts, Vec3::new(0.0, 1.0, 0.7), Vec3::ZERO, std::f32::consts::FRAC_PI_2, 1.0, 1.4, false));
        // A dying body hides within its height, where it may lie.
        assert!(eye_inside(&parts, Vec3::new(1.2, 0.5, 0.0), Vec3::ZERO, 0.0, 1.0, 1.4, true));
        assert!(!eye_inside(&parts, Vec3::new(2.0, 0.5, 0.0), Vec3::ZERO, 0.0, 1.0, 1.4, true));
    }

    #[test]
    fn rays_find_boxes_in_front_only() {
        let aabb = Aabb { min: Vec3::new(2.0, -0.5, -0.5), max: Vec3::new(3.0, 0.5, 0.5) };
        assert_eq!(ray_box(Vec3::ZERO, Vec3::X, &aabb), Some(2.0));
        assert_eq!(ray_box(Vec3::ZERO, Vec3::NEG_X, &aabb), None);
        assert_eq!(ray_box(Vec3::ZERO, Vec3::Y, &aabb), None);
    }
}
