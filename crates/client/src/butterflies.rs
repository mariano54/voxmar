//! Butterflies and moths: the ambient sets with wings (`ambient.ron`, `wings`), flown about
//! the player by the client alone. Where the land's air brings a set out (its climate and
//! biome, the hour, the weather and the day, as `weather::atmosphere` works out for every
//! ambient set), a few of it flutter about at their height over the ground. They go to the
//! blocks they visit (flowers) and settle on them a while, opening and closing their wings;
//! the ones drawn to light (moths) gather and circle about torches, lamps and fires; a
//! player who comes close sends them off; the wind carries them; and when their hour or the
//! dry weather ends they fly away. Nobody else sees the same ones and nothing can touch
//! them: there is nothing to keep in step with the server.
//!
//! What they visit and the lights they seek are found by a sweep of the blocks around the
//! player, a few rows a frame. All of them are drawn as one mesh, two wing quads each in a
//! pixel pattern painted from the set's colours (`paint_wing`), rebuilt each frame on the
//! CPU: a dozen butterflies are a hundred vertices.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use voxmar_engine::block::BlockId;
use voxmar_engine::creature::{mix, unit_from};
use voxmar_worldgen::{ParticleDef, WingPattern, WingsDef};

use crate::breaking::sky_light;
use crate::drops::block_light;
use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::render::SceneLight;
use crate::weather::ClientWeather;
use crate::weather::atmosphere::{AtmosphereSystems, ClientAtmosphere};
use crate::world::ClientWorld;

pub struct ButterfliesPlugin;

impl Plugin for ButterfliesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Butterflies>()
            .add_systems(OnEnter(Screen::InGame), setup)
            .add_systems(Update, fly.after(AtmosphereSystems).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut butterflies: ResMut<Butterflies>| *butterflies = Butterflies::default());
    }
}

/// Pixels across one wing's painting, and how many sets the atlas holds at once.
const WING: u32 = 16;
const SLOTS: u32 = 16;
/// Blocks from the player they appear at, and beyond which they are gone.
const APPEAR: (f32, f32) = (6.0, 18.0);
const GONE: f32 = 30.0;
/// Seconds between one appearing and the next.
const APPEAR_EVERY: f32 = 0.6;
/// How near a player comes before they fly off, in blocks, and for how long they flee.
const STARTLE: f32 = 2.5;
const FLEES: f32 = 2.0;
/// Seconds they take to come and go.
const FADE: f32 = 1.5;
/// How far about the player the sweep looks for what they visit and the lights they seek
/// (blocks across and up from below the feet), and how many it keeps of each.
const SWEEP: i32 = 12;
const SWEEP_DOWN: i32 = 6;
const SWEEP_UP: i32 = 8;
const SWEEP_KEEPS: usize = 96;
/// Block light a moth seeks: the least that counts as a light, and how near it keeps.
const LIGHT_LEAST: u8 = 8;
const LIGHT_ORBIT: f32 = 1.3;
/// Seconds between working out each one's light (staggered).
const RELIGHT: f32 = 0.25;
/// Wingbeats a second on the wing, and opening and closing at rest.
const BEATS: f32 = 9.0;
const RESTING_BEATS: f32 = 0.4;

#[derive(Resource, Default)]
struct Butterflies {
    flying: Vec<Butterfly>,
    /// The sets seen so far in this game: their atlas slot and what they visit.
    sets: Vec<Set>,
    drawn: Option<Drawn>,
    sweep: Sweep,
    since_appear: f32,
    count: u64,
    /// Wing quads drawn last frame.
    pub quads: usize,
}

struct Drawn {
    mesh: Handle<Mesh>,
    atlas: Handle<Image>,
    entity: Entity,
}

struct Set {
    name: String,
    def: WingsDef,
    size: f32,
    slot: u32,
    /// Whether it visits each block, by id.
    visits: Vec<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Flutter,
    /// Heading to a block it visits, to settle at this point on it.
    Visiting(Vec3),
    /// Settled for this many seconds more.
    Settled(f32),
    /// Fleeing a player for this many seconds more.
    Startled(f32),
    /// Its time is over: it flies off and fades.
    Leaving,
}

#[derive(Clone, Debug)]
struct Butterfly {
    set: usize,
    position: Vec3,
    velocity: Vec3,
    heading: f32,
    state: State,
    seed: u64,
    age: f32,
    /// Seconds since it began to leave.
    leaving: f32,
    /// Its wingbeat, in beats.
    beat: f32,
    light: Vec3,
    relight: f32,
    /// Seconds until it next looks for something to visit.
    restless: f32,
    /// Seconds it stays whether its set is out or not (one the gate releases).
    kept: f32,
}

/// What the last full sweep found: blocks that are visited, and lights.
#[derive(Default)]
struct Sweep {
    centre: IVec3,
    row: i32,
    visits: Vec<(IVec3, BlockId)>,
    lights: Vec<Vec3>,
    found_visits: Vec<(IVec3, BlockId)>,
    found_lights: Vec<Vec3>,
    /// Whether anything out now visits blocks or seeks light.
    wanted: bool,
}

fn nearest_image() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor { mag_filter: ImageFilterMode::Nearest, min_filter: ImageFilterMode::Nearest, ..default() })
}

/// The atlas, the material and the mesh, made once a game; the material's pipeline is warmed
/// so the first butterfly compiles nothing.
fn setup(
    mut commands: Commands,
    mut butterflies: ResMut<Butterflies>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let extent = Extent3d { width: WING * SLOTS, height: WING, depth_or_array_layers: 1 };
    let mut atlas = Image::new(extent, TextureDimension::D2, vec![0; (WING * SLOTS * WING * 4) as usize], TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    atlas.sampler = nearest_image();
    let atlas = images.add(atlas);
    let material = materials.add(StandardMaterial {
        base_color_texture: Some(atlas.clone()),
        alpha_mode: AlphaMode::Mask(0.5),
        unlit: true,
        cull_mode: None,
        double_sided: true,
        ..default()
    });
    let mesh = meshes.add(wing_mesh(&[]));
    crate::render::warm::warm(&mut commands, mesh.clone(), material.clone(), false, DespawnOnExit(Screen::InGame));
    let entity = commands
        .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material), Transform::default(), Visibility::Hidden, NoFrustumCulling, NotShadowCaster, DespawnOnExit(Screen::InGame)))
        .id();
    butterflies.drawn = Some(Drawn { mesh, atlas, entity });
}

/// One wing's corners and colour, for the mesh.
struct Quad {
    corners: [Vec3; 4],
    /// Atlas slot.
    slot: u32,
    colour: [f32; 4],
}

/// The mesh of these quads: positions, normals, atlas coordinates and colours. Never empty,
/// so its layout is always the same: no quads give one too small to see.
fn wing_mesh(quads: &[Quad]) -> Mesh {
    let none = [Quad { corners: [Vec3::ZERO; 4], slot: 0, colour: [0.0; 4] }];
    let quads = if quads.is_empty() { &none[..] } else { quads };
    let mut positions = Vec::with_capacity(quads.len() * 4);
    let mut uvs = Vec::with_capacity(quads.len() * 4);
    let mut colours = Vec::with_capacity(quads.len() * 4);
    let mut indices = Vec::with_capacity(quads.len() * 6);
    for quad in quads {
        let base = positions.len() as u32;
        positions.extend(quad.corners.map(|c| c.to_array()));
        // The hinge is u = 0, the tip 1; the front v = 0.
        let (u0, u1) = (quad.slot as f32 / SLOTS as f32, (quad.slot + 1) as f32 / SLOTS as f32);
        uvs.extend([[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]]);
        colours.extend([quad.colour; 4]);
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
        .with_inserted_indices(Indices::U32(indices))
}

/// Whether a point of one wing (`x` from the hinge to the tip, `y` from the front to the
/// back, both 0..1) is wing: a broad forewing over a smaller hindwing.
fn on_wing(x: f32, y: f32) -> bool {
    let ellipse = |cx: f32, cy: f32, rx: f32, ry: f32| ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0;
    ellipse(0.42, 0.33, 0.6, 0.31) || ellipse(0.3, 0.72, 0.36, 0.27)
}

/// One wing painted in pixels, `WING` square, row by row from the front: the set's colour
/// with its pattern in `marks`, a dark body along the hinge, clear outside the wing.
pub fn paint_wing(ground: (u8, u8, u8), marks: (u8, u8, u8), pattern: WingPattern) -> Vec<[u8; 4]> {
    let n = WING as i32;
    let ground = [ground.0, ground.1, ground.2];
    let marks = [marks.0, marks.1, marks.2];
    let dark = |c: [u8; 3], by: f32| c.map(|v| (v as f32 * by) as u8);
    let at = |i: i32| (i as f32 + 0.5) / n as f32;
    let mut pixels = vec![[0u8; 4]; (n * n) as usize];
    for py in 0..n {
        for px in 0..n {
            let (x, y) = (at(px), at(py));
            if !on_wing(x, y) {
                continue;
            }
            let edge = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|&(dx, dy)| !on_wing(at(px + dx), at(py + dy)));
            let near = |cx: f32, cy: f32, r: f32| (x - cx).hypot(y - cy) < r;
            let mut colour = match pattern {
                WingPattern::Plain => ground,
                WingPattern::Spots if near(0.62, 0.26, 0.11) || near(0.36, 0.7, 0.09) => marks,
                WingPattern::Band if (0.42..0.6).contains(&(x + 0.25 * y)) => marks,
                WingPattern::Eyes if near(0.56, 0.32, 0.07) || near(0.34, 0.72, 0.05) => dark(ground, 0.2),
                WingPattern::Eyes if near(0.56, 0.32, 0.15) || near(0.34, 0.72, 0.1) => marks,
                WingPattern::Tips if x > 0.72 && y < 0.6 => marks,
                WingPattern::Veins if (y.atan2(x + 0.05) * 5.0).fract() < 0.18 && x > 0.15 => marks,
                _ => ground,
            };
            if edge {
                colour = if pattern == WingPattern::Plain { marks } else { dark(colour, 0.6) };
            }
            if px == 0 {
                colour = [36, 30, 28];
            }
            pixels[(py * n + px) as usize] = [colour[0], colour[1], colour[2], 255];
        }
    }
    pixels
}

/// The set's index, adding it (and painting its wings into the atlas) the first time.
fn set_of(butterflies: &mut Butterflies, set: &ParticleDef, world: &ClientWorld, images: &mut Assets<Image>) -> Option<usize> {
    if let Some(i) = butterflies.sets.iter().position(|s| s.name == set.name) {
        return Some(i);
    }
    let def = set.wings.clone()?;
    let slot = butterflies.sets.len() as u32;
    if slot >= SLOTS {
        return None;
    }
    let registry = &world.registry;
    let visits = registry
        .ids()
        .map(|id| {
            let block = registry.def(id);
            def.visits.iter().any(|v| match v.strip_prefix("tag:") {
                Some(tag) => block.has_tag(tag),
                None => registry.id(v) == Some(id),
            })
        })
        .collect();
    if let Some(drawn) = &butterflies.drawn
        && let Some(mut atlas) = images.get_mut(&drawn.atlas)
        && let Some(data) = atlas.data.as_mut()
    {
        let painted = paint_wing(set.color, def.marks, def.pattern);
        for y in 0..WING {
            for x in 0..WING {
                let at = ((y * WING * SLOTS + slot * WING + x) * 4) as usize;
                data[at..at + 4].copy_from_slice(&painted[(y * WING + x) as usize]);
            }
        }
    }
    butterflies.sets.push(Set { name: set.name.clone(), def, size: set.size, slot, visits });
    Some(butterflies.sets.len() - 1)
}

/// The ground under (x, z) near height `y`: the top of the first solid block or water below,
/// and whether it is water.
fn ground(world: &ClientWorld, x: i32, z: i32, y: i32) -> Option<(f32, bool)> {
    (0..32).map(|d| IVec3::new(x, y + 12 - d, z)).find_map(|cell| {
        let block = world.block(cell)?;
        let info = world.registry.info(block);
        (info.solid || info.fluid.is_some()).then(|| (cell.y as f32 + 1.0, info.fluid.is_some()))
    })
}

/// Sweeps a few rows of the blocks about the player for what is visited and for lights.
fn sweep(sweep: &mut Sweep, world: &ClientWorld, sets: &[Set], player: IVec3) {
    let rows = 3;
    for _ in 0..rows {
        if sweep.row > 2 * SWEEP {
            sweep.visits = std::mem::take(&mut sweep.found_visits);
            sweep.lights = std::mem::take(&mut sweep.found_lights);
            sweep.row = 0;
            sweep.centre = player;
        }
        let x = sweep.centre.x - SWEEP + sweep.row;
        sweep.row += 1;
        for z in sweep.centre.z - SWEEP..=sweep.centre.z + SWEEP {
            for y in sweep.centre.y - SWEEP_DOWN..=sweep.centre.y + SWEEP_UP {
                let cell = IVec3::new(x, y, z);
                let Some(block) = world.block(cell) else { continue };
                if block == BlockId::AIR {
                    continue;
                }
                // A solid one (blossom) only where its top is open to land on.
                let visited = sets.iter().any(|s| s.visits.get(block.0 as usize).copied().unwrap_or(false));
                if visited && sweep.found_visits.len() < SWEEP_KEEPS && !(world.registry.info(block).solid && world.is_solid(cell + IVec3::Y)) {
                    sweep.found_visits.push((cell, block));
                }
                if sweep.found_lights.len() < SWEEP_KEEPS && world.registry.info(block).emission >= LIGHT_LEAST {
                    sweep.found_lights.push(cell.as_vec3() + 0.5);
                }
            }
        }
    }
}

/// A random number in 0..1 from a butterfly's seed and a salt.
fn random(seed: u64, salt: u64) -> f32 {
    unit_from(mix(seed, salt))
}

/// Smooth noise in 0..1 along `t`, its own for each seed.
fn wobble(seed: u64, salt: u64, t: f32) -> f32 {
    let (i, f) = (t.floor(), t.fract());
    let a = random(seed, salt ^ (i as i64 as u64));
    let b = random(seed, salt ^ ((i + 1.0) as i64 as u64));
    a + (b - a) * f * f * (3.0 - 2.0 * f)
}

/// What one butterfly does this frame: where it heads and how fast, given what is about.
#[derive(Clone, Copy)]
struct Near<'a> {
    player: Vec3,
    wind: Vec3,
    /// Visited blocks about, with the point on each it settles at.
    visits: &'a [(Vec3, BlockId)],
    lights: &'a [Vec3],
    /// The ground under it and whether that is water.
    ground: Option<(f32, bool)>,
    /// Whether its set is still wanted here and now.
    wanted: bool,
}

/// Moves one butterfly on by `dt` seconds. Pure: everything it reads is in `near`, so it can
/// be tested without a world.
fn step(b: &mut Butterfly, set: &Set, near: Near, solid: impl Fn(IVec3) -> bool, dt: f32) {
    b.age += dt;
    let def = &set.def;
    if !near.wanted && b.state != State::Leaving {
        b.state = State::Leaving;
    }
    // A player close by sends it off, settled or not.
    let away = (b.position - near.player).with_y(0.0);
    if b.state != State::Leaving && away.length() < STARTLE && !matches!(b.state, State::Startled(_)) {
        b.state = State::Startled(FLEES);
        b.heading = away.z.atan2(away.x);
    }
    let ground = near.ground.map_or(b.position.y - def.height.0, |g| g.0);
    let speed = def.speed * (0.7 + 0.6 * wobble(b.seed, 3, b.age * 0.7));
    let mut wish;
    let mut height = ground + def.height.0 + (def.height.1 - def.height.0) * wobble(b.seed, 5, b.age * 0.25);
    match b.state {
        State::Settled(left) => {
            b.velocity = Vec3::ZERO;
            b.state = if left > dt { State::Settled(left - dt) } else { State::Flutter };
            b.beat += dt * RESTING_BEATS;
            return;
        }
        State::Startled(left) => {
            b.state = if left > dt { State::Startled(left - dt) } else { State::Flutter };
            wish = Vec3::new(b.heading.cos(), 0.0, b.heading.sin()) * def.speed * 2.0;
            height = b.position.y.max(ground + def.height.1) + 1.0;
        }
        State::Leaving => {
            b.leaving += dt;
            let off = if away.length() > 0.1 { away.normalize() } else { Vec3::X };
            wish = off * def.speed * 1.5;
            height = b.position.y + 2.0;
        }
        State::Visiting(to) => {
            let to_it = to - b.position;
            if to_it.length() < 0.2 {
                b.position = to;
                b.state = State::Settled(3.0 + 6.0 * random(b.seed, b.age.to_bits() as u64));
                b.restless = 6.0 + 10.0 * random(b.seed, 11 ^ b.age.to_bits() as u64);
                b.velocity = Vec3::ZERO;
                return;
            }
            // Wavering on its way, slowing as it comes in.
            wish = to_it.normalize() * def.speed.min(to_it.length() * 2.0 + 0.3);
            height = to.y + to_it.with_y(0.0).length().min(1.0) * 0.5;
            if to_it.length() > 10.0 {
                b.state = State::Flutter;
            }
        }
        State::Flutter => {
            b.heading += (wobble(b.seed, 7, b.age * 1.3) - 0.5) * 6.0 * dt;
            wish = Vec3::new(b.heading.cos(), 0.0, b.heading.sin()) * speed;
            // Moths keep about a light; others now and then go to a block they visit.
            if def.to_light
                && let Some(light) = near.lights.iter().min_by(|a, b2| a.distance_squared(b.position).total_cmp(&b2.distance_squared(b.position)))
                && light.distance(b.position) < 14.0
            {
                let angle = b.age * 1.7 + random(b.seed, 13) * 6.3;
                let round = *light + Vec3::new(angle.cos(), 0.4 * (b.age * 2.3).sin(), angle.sin()) * LIGHT_ORBIT;
                wish = (round - b.position).normalize_or_zero() * def.speed * 1.2;
                height = round.y;
            } else {
                b.restless -= dt;
                if b.restless <= 0.0 {
                    b.restless = 2.0 + 4.0 * random(b.seed, 17 ^ b.age.to_bits() as u64);
                    let pick = near
                        .visits
                        .iter()
                        .filter(|(at, _)| at.distance(b.position) < 8.0)
                        .min_by(|(a, _), (c, _)| (a.distance(b.position) + random(b.seed, a.x.to_bits() as u64) * 4.0).total_cmp(&(c.distance(b.position) + random(b.seed, c.x.to_bits() as u64) * 4.0)));
                    if let Some(&(to, _)) = pick {
                        b.state = State::Visiting(to);
                    }
                }
            }
            // It never strays far over water.
            if near.ground.is_some_and(|g| g.1) {
                b.heading += std::f32::consts::PI * dt;
            }
        }
    }
    // Fluttering bobs it up and down, less as it comes in to land.
    let bob = if matches!(b.state, State::Visiting(_)) { 0.15 } else { 0.6 };
    let climb = ((height - b.position.y) * 1.5).clamp(-1.5, 2.0) + (b.age * 6.0 + random(b.seed, 19) * 6.3).sin() * bob;
    let target = wish.with_y(climb) + near.wind * 0.25;
    b.velocity = b.velocity.lerp(target, (4.0 * dt).min(1.0));
    let next = b.position + b.velocity * dt;
    if solid(next.floor().as_ivec3()) {
        // It turns away from what it meets, and rises.
        b.heading += std::f32::consts::PI;
        b.velocity = Vec3::Y * 0.8;
        if !solid((b.position + Vec3::Y * 0.3).floor().as_ivec3()) {
            b.position.y += 0.3 * dt;
        }
    } else {
        b.position = next;
    }
    b.beat += dt * BEATS * (0.8 + 0.4 * wobble(b.seed, 23, b.age));
}

/// How far open a butterfly's wings are, in radians above the level: beating in flight,
/// slowly opening and closing at rest.
fn wing_angle(b: &Butterfly) -> f32 {
    let wave = (b.beat * std::f32::consts::TAU).sin();
    match b.state {
        State::Settled(_) => 1.45 - 0.9 * (0.5 + 0.5 * wave),
        _ => 0.35 + 0.95 * wave,
    }
}

/// Too many of a set (its hour passing, rain coming): the extra ones leave. One kept on purpose
/// (`release`) stays its time out, wanted or not.
fn send_off_extras(flying: &mut [Butterfly], wanted: &[(usize, usize)]) {
    for &(set, want) in wanted {
        let staying = flying.iter().filter(|b| b.set == set && b.state != State::Leaving).count();
        for b in flying.iter_mut().filter(|b| b.set == set && b.state != State::Leaving && b.kept <= 0.0).take(staying.saturating_sub(want)) {
            b.state = State::Leaving;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fly(
    time: Res<Time>,
    mut butterflies: ResMut<Butterflies>,
    air: Res<ClientAtmosphere>,
    weather: Res<ClientWeather>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    scene_light: Res<SceneLight>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut visibility: Query<&mut Visibility>,
) {
    let dt = time.delta_secs().min(0.1);
    let butterflies = &mut *butterflies;
    if !player.joined {
        return;
    }
    // How many of each set are wanted now.
    let mut wanted: Vec<(usize, usize)> = Vec::new();
    for out in &air.wings {
        let want = out.count.round() as usize;
        if (want > 0 || butterflies.sets.iter().any(|s| s.name == out.set.name))
            && let Some(set) = set_of(butterflies, &out.set, &world, &mut images)
        {
            wanted.push((set, want));
        }
    }
    let want_of = |set: usize| wanted.iter().find(|w| w.0 == set).map_or(0, |w| w.1);
    let feet = player.position.floor().as_ivec3();
    butterflies.sweep.wanted = wanted.iter().any(|&(set, want)| want > 0 && (butterflies.sets[set].def.to_light || !butterflies.sets[set].def.visits.is_empty()));
    if butterflies.sweep.wanted {
        sweep(&mut butterflies.sweep, &world, &butterflies.sets, feet);
    }

    // One more of a set short of what is wanted, now and then: near what it visits when there
    // is any, else somewhere about at its height.
    butterflies.since_appear += dt;
    if butterflies.since_appear >= APPEAR_EVERY
        && let Some(&(set, _)) = wanted.iter().find(|&&(set, want)| butterflies.flying.iter().filter(|b| b.set == set && b.state != State::Leaving).count() < want)
    {
        butterflies.since_appear = 0.0;
        butterflies.count += 1;
        let seed = mix(mix(air.seed().unwrap_or(0), butterflies.count), set as u64);
        let def = &butterflies.sets[set].def;
        let visited: Vec<IVec3> = butterflies.sweep.visits.iter().filter(|(_, block)| butterflies.sets[set].visits[block.0 as usize]).map(|(at, _)| *at).collect();
        let column = if !visited.is_empty() && random(seed, 1) < 0.8 {
            let at = visited[(random(seed, 2) * visited.len() as f32) as usize % visited.len()];
            at.xz() + IVec2::new((random(seed, 3) * 7.0) as i32 - 3, (random(seed, 4) * 7.0) as i32 - 3)
        } else {
            let angle = random(seed, 5) * std::f32::consts::TAU;
            let distance = APPEAR.0 + random(seed, 6) * (APPEAR.1 - APPEAR.0);
            (player.position.xz() + Vec2::new(angle.cos(), angle.sin()) * distance).floor().as_ivec2()
        };
        if let Some((top, false)) = ground(&world, column.x, column.y, feet.y)
            && column.as_vec2().distance(player.position.xz()) > STARTLE * 2.0
        {
            let height = def.height.0 + random(seed, 7) * (def.height.1 - def.height.0);
            butterflies.flying.push(Butterfly {
                set,
                position: Vec3::new(column.x as f32 + 0.5, top + height, column.y as f32 + 0.5),
                velocity: Vec3::ZERO,
                heading: random(seed, 8) * std::f32::consts::TAU,
                state: State::Flutter,
                seed,
                age: 0.0,
                leaving: 0.0,
                beat: random(seed, 9),
                light: Vec3::ZERO,
                relight: random(seed, 10) * RELIGHT,
                restless: 1.0 + 3.0 * random(seed, 11),
                kept: 0.0,
            });
        }
    }
    send_off_extras(&mut butterflies.flying, &wanted);

    let wind = Vec3::new(weather.current.wind.x, 0.0, weather.current.wind.y);
    let visits: Vec<(Vec3, BlockId)> = butterflies
        .sweep
        .visits
        .iter()
        .map(|&(at, block)| {
            // On top of a solid block (blossom), among a plant's blooms.
            let info = world.registry.info(block);
            let up = if info.solid { 1.02 } else { info.plant_height.min(1.0) * 0.8 };
            (at.as_vec3() + Vec3::new(0.5, up, 0.5), block)
        })
        .collect();
    let mut quads = Vec::with_capacity(butterflies.flying.len() * 2);
    let sets = &butterflies.sets;
    butterflies.flying.retain_mut(|b| {
        let set = &sets[b.set];
        let own: Vec<(Vec3, BlockId)> = visits.iter().filter(|(_, block)| set.visits[block.0 as usize]).copied().collect();
        let column = b.position.floor().as_ivec3();
        let near = Near {
            player: player.position,
            wind,
            visits: &own,
            lights: &butterflies.sweep.lights,
            ground: ground(&world, column.x, column.z, column.y),
            wanted: want_of(b.set) > 0 || b.kept > 0.0,
        };
        b.kept -= dt;
        step(b, set, near, |cell| world.is_solid(cell), dt);
        if b.position.distance(player.position) > GONE || b.leaving > FADE {
            return false;
        }
        b.relight -= dt;
        if b.relight <= 0.0 {
            b.relight = RELIGHT;
            let cell = b.position.floor().as_ivec3();
            let light = scene_light.at(sky_light(&world, cell), block_light(&world, cell));
            b.light = Vec3::new(light.red, light.green, light.blue);
        }
        // It grows in as it comes and shrinks away as it goes.
        let shown = (b.age / FADE).min(1.0) * (1.0 - b.leaving / FADE).max(0.0);
        let span = set.size * shown;
        let travel = b.velocity.with_y(0.0);
        let forward = if travel.length() > 0.05 { travel.normalize() } else { Vec3::new(b.heading.cos(), 0.0, b.heading.sin()) };
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let angle = wing_angle(b);
        for side in [-1.0, 1.0] {
            let out = (right * side * angle.cos() + Vec3::Y * angle.sin()) * span * 0.5;
            let (front, back) = (forward * span * 0.42, -forward * span * 0.42);
            quads.push(Quad {
                corners: [b.position + front, b.position + front + out, b.position + back + out, b.position + back],
                slot: set.slot,
                colour: [b.light.x, b.light.y, b.light.z, 1.0],
            });
        }
        true
    });

    butterflies.quads = quads.len();
    let Some(drawn) = &butterflies.drawn else { return };
    if let Ok(mut shows) = visibility.get_mut(drawn.entity) {
        let want = if quads.is_empty() { Visibility::Hidden } else { Visibility::Inherited };
        if *shows != want {
            *shows = want;
        }
    }
    if !quads.is_empty()
        && let Some(mut mesh) = meshes.get_mut(&drawn.mesh)
    {
        *mesh = wing_mesh(&quads);
    }
}

/// Lets one of the set named `set` (`base:cabbage_white`) fly at `at` for `seconds`, whether
/// its set is out there or not; for the gate. False if there is no such winged set yet.
pub fn release(world: &mut World, set: &str, at: Vec3, seconds: f32) -> bool {
    let Some(def) = world.resource::<ClientAtmosphere>().particle_set(set) else { return false };
    world.resource_scope(|world, mut butterflies: Mut<Butterflies>| {
        let index = world.resource_scope(|world, mut images: Mut<Assets<Image>>| set_of(&mut butterflies, &def, world.resource::<ClientWorld>(), &mut images));
        let Some(index) = index else { return false };
        let seed = mix(0x0b07_7e2f, butterflies.count);
        butterflies.flying.push(Butterfly {
            set: index,
            position: at,
            velocity: Vec3::ZERO,
            heading: 0.0,
            state: State::Flutter,
            seed,
            age: FADE,
            leaving: 0.0,
            beat: 0.0,
            light: Vec3::ONE,
            relight: 0.0,
            restless: 1.0,
            kept: seconds,
        });
        true
    })
}

/// Wing quads drawn last frame: two a butterfly.
pub fn drawn(world: &World) -> usize {
    world.resource::<Butterflies>().quads
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(def: WingsDef) -> Set {
        Set { name: "test:butterfly".into(), def, size: 0.3, slot: 0, visits: vec![false, true] }
    }

    fn wings() -> WingsDef {
        WingsDef { most: 4, pattern: WingPattern::Spots, marks: (20, 20, 20), height: (0.5, 2.0), speed: 1.5, visits: vec!["tag:flower".into()], to_light: false }
    }

    fn butterfly(at: Vec3) -> Butterfly {
        Butterfly { set: 0, position: at, velocity: Vec3::ZERO, heading: 0.0, state: State::Flutter, seed: 7, age: 0.0, leaving: 0.0, beat: 0.0, light: Vec3::ONE, relight: 0.0, restless: 0.0, kept: 0.0 }
    }

    fn near<'a>(visits: &'a [(Vec3, BlockId)], lights: &'a [Vec3]) -> Near<'a> {
        Near { player: Vec3::new(100.0, 0.0, 100.0), wind: Vec3::ZERO, visits, lights, ground: Some((0.0, false)), wanted: true }
    }

    fn open(_: IVec3) -> bool {
        false
    }

    #[test]
    fn a_kept_butterfly_stays_when_its_set_is_not_wanted() {
        // A released one (the gate's) whose set is out of season: the free one leaves, the kept one stays.
        let mut flying = vec![Butterfly { kept: 30.0, ..butterfly(Vec3::ZERO) }, butterfly(Vec3::X)];
        send_off_extras(&mut flying, &[(0, 0)]);
        assert_ne!(flying[0].state, State::Leaving, "the kept one was sent off");
        assert_eq!(flying[1].state, State::Leaving);
        // Kept time over, it goes like the rest.
        flying[0].kept = 0.0;
        send_off_extras(&mut flying, &[(0, 0)]);
        assert_eq!(flying[0].state, State::Leaving);
        // Wanted ones stay.
        let mut flying = vec![butterfly(Vec3::ZERO), butterfly(Vec3::X)];
        send_off_extras(&mut flying, &[(0, 1)]);
        assert_eq!(flying.iter().filter(|b| b.state == State::Leaving).count(), 1);
    }

    #[test]
    fn wings_are_painted_in_their_pattern_with_a_clear_outline() {
        let pixels = paint_wing((240, 160, 40), (20, 20, 20), WingPattern::Spots);
        assert_eq!(pixels.len(), (WING * WING) as usize);
        let clear = pixels.iter().filter(|p| p[3] == 0).count();
        let ground = pixels.iter().filter(|p| p[..3] == [240, 160, 40]).count();
        let marks = pixels.iter().filter(|p| p[..3] == [20, 20, 20]).count();
        assert!(clear > 20 && ground > 60 && marks > 4, "clear {clear}, ground {ground}, marks {marks}");
        // Every pattern differs from plain.
        let plain = paint_wing((240, 160, 40), (20, 20, 20), WingPattern::Plain);
        for pattern in [WingPattern::Spots, WingPattern::Band, WingPattern::Eyes, WingPattern::Tips, WingPattern::Veins] {
            assert_ne!(paint_wing((240, 160, 40), (20, 20, 20), pattern), plain, "{pattern:?}");
        }
    }

    #[test]
    fn they_flutter_at_their_height_and_never_into_blocks() {
        let set = set(wings());
        let mut b = butterfly(Vec3::new(0.5, 1.0, 0.5));
        // A wall at x = 4.
        let wall = |cell: IVec3| cell.x == 4 || cell.y < 0;
        for _ in 0..1200 {
            step(&mut b, &set, near(&[], &[]), wall, 1.0 / 60.0);
            assert!(!wall(b.position.floor().as_ivec3()), "flew into the wall at {}", b.position);
            assert!((0.0..4.0).contains(&b.position.y), "strayed to height {}", b.position.y);
        }
    }

    #[test]
    fn they_settle_on_flowers_and_a_player_sends_them_off() {
        let set = set(wings());
        let flower = Vec3::new(3.5, 0.8, 0.5);
        let visits = [(flower, BlockId(1))];
        let mut b = butterfly(Vec3::new(0.5, 1.5, 0.5));
        let mut settled = false;
        for _ in 0..600 {
            step(&mut b, &set, near(&visits, &[]), open, 1.0 / 60.0);
            if matches!(b.state, State::Settled(_)) {
                settled = true;
                break;
            }
        }
        assert!(settled && b.position.distance(flower) < 1e-3, "never settled: {:?} at {}", b.state, b.position);
        // Settled, its wings open and close slowly; a player stepping close sends it off.
        let before = b.position;
        step(&mut b, &set, Near { player: flower + Vec3::X, ..near(&visits, &[]) }, open, 1.0 / 60.0);
        assert!(matches!(b.state, State::Startled(_)));
        for _ in 0..60 {
            step(&mut b, &set, Near { player: flower + Vec3::X, ..near(&visits, &[]) }, open, 1.0 / 60.0);
        }
        assert!(b.position.x < before.x - 0.5, "fled toward the player: {}", b.position);
    }

    #[test]
    fn moths_gather_at_lights() {
        let set = set(WingsDef { to_light: true, visits: vec![], ..wings() });
        let lamp = Vec3::new(6.5, 2.5, 6.5);
        let mut b = butterfly(Vec3::new(0.5, 1.0, 0.5));
        let mut nearest = f32::MAX;
        for i in 0..1800 {
            step(&mut b, &set, near(&[], &[lamp]), open, 1.0 / 60.0);
            if i > 900 {
                nearest = nearest.min(b.position.distance(lamp));
                assert!(b.position.distance(lamp) < LIGHT_ORBIT + 2.0, "wandered from the light: {}", b.position.distance(lamp));
            }
        }
        assert!(nearest < LIGHT_ORBIT + 0.8, "never came near the light: {nearest}");
    }

    #[test]
    fn rain_or_their_hour_ending_sends_them_away() {
        let set = set(wings());
        let mut b = butterfly(Vec3::new(0.5, 1.0, 0.5));
        let start = b.position;
        for _ in 0..60 {
            step(&mut b, &set, Near { wanted: false, ..near(&[], &[]) }, open, 1.0 / 60.0);
        }
        assert_eq!(b.state, State::Leaving);
        assert!(b.leaving >= 1.0 - 1e-3 && b.position.distance(start) > 0.5);
    }
}
