//! Creature skins: a pixel texture for every individual, painted from its species' colours
//! and markings and its own seed, laid out over its boxes the way Minecraft's entity
//! textures are (each face of each box gets its own patch).
//!
//! The painter works in model space: each texel knows where on the body it lies, so a
//! belly line, a dorsal stripe, spots or a saddle run on across faces and boxes instead of
//! restarting on each. On that ground it puts what makes an animal read as one at a glance:
//! eyes with a catchlight (on the sides of a prey animal's head, in front on a hunter's,
//! a cluster on a spider's), nostrils, a hoof or paw at the foot of each leg, the pale
//! inside of an ear, feathers in rows, rings on a shell, and fur that is never one flat
//! colour. Every face gets a slightly darker rim, so the boxes read as solid shapes.
//!
//! The layout depends only on the body plan, so a species' meshes are made once; the paint
//! depends on the individual's seed too (where its spots fall, its fur's grain), so no two
//! look the same. Genes will add to what the seed varies (docs/breeding.md).

use glam::Vec3;
use serde::Deserialize;

use super::model::{Part, Role};
use super::{BodyDef, mix, unit_from};

/// Texels per block.
pub const TEXELS: f32 = 32.0;

/// A patch of the texture, in texels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A face of a box: its outward normal and the axes its texture's u (rightward) and v
/// (downward) run along, as seen from outside. The client lays out its meshes by these.
#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub normal: Vec3,
    pub u: Vec3,
    pub v: Vec3,
}

pub const FACES: [Face; 6] = [
    Face { normal: Vec3::Y, u: Vec3::X, v: Vec3::Z },
    Face { normal: Vec3::NEG_Y, u: Vec3::X, v: Vec3::NEG_Z },
    Face { normal: Vec3::X, u: Vec3::NEG_Z, v: Vec3::NEG_Y },
    Face { normal: Vec3::NEG_X, u: Vec3::Z, v: Vec3::NEG_Y },
    Face { normal: Vec3::Z, u: Vec3::X, v: Vec3::NEG_Y },
    Face { normal: Vec3::NEG_Z, u: Vec3::NEG_X, v: Vec3::NEG_Y },
];

/// The index in [`FACES`] of a box's front (−Z).
const FRONT: usize = 5;

/// Where each face of each box lies in a species' texture.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    /// Per part, per face (in [`FACES`] order).
    pub faces: Vec<[Rect; 6]>,
}

/// One texel of gutter around every patch, filled with the patch's edge, so sampling at a
/// patch's border never picks up its neighbour.
const GUTTER: u32 = 1;

/// The size of a face's patch, in texels.
fn patch(size: Vec3, face: &Face) -> (u32, u32) {
    let along = |axis: Vec3| (axis.abs().dot(size) * TEXELS).round().max(1.0) as u32;
    (along(face.u), along(face.v))
}

/// Packs every face of every part into one texture, tallest patches first, row by row.
pub fn layout(parts: &[Part]) -> Layout {
    let mut patches: Vec<(usize, usize, u32, u32)> = Vec::new();
    for (p, part) in parts.iter().enumerate() {
        for (f, face) in FACES.iter().enumerate() {
            let (w, h) = patch(part.size, face);
            patches.push((p, f, w, h));
        }
    }
    let area: u32 = patches.iter().map(|&(_, _, w, h)| (w + 2 * GUTTER) * (h + 2 * GUTTER)).sum();
    let widest = patches.iter().map(|&(_, _, w, _)| w + 2 * GUTTER).max().unwrap_or(1);
    let width = ((area as f32 * 1.2).sqrt().ceil() as u32).max(widest).max(32).next_power_of_two();
    patches.sort_by(|a, b| b.3.cmp(&a.3).then(b.2.cmp(&a.2)));
    let mut faces = vec![[Rect::default(); 6]; parts.len()];
    let (mut x, mut y, mut row) = (0, 0, 0);
    for (p, f, w, h) in patches {
        let (gw, gh) = (w + 2 * GUTTER, h + 2 * GUTTER);
        if x + gw > width {
            x = 0;
            y += row;
            row = 0;
        }
        faces[p][f] = Rect { x: x + GUTTER, y: y + GUTTER, w, h };
        x += gw;
        row = row.max(gh);
    }
    Layout { width, height: (y + row).max(1), faces }
}

/// Where a species' eyes are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Eyes {
    /// On the sides of the head, to watch all round: prey.
    #[default]
    Side,
    /// In front, to judge a leap: hunters.
    Front,
    /// Several pairs in front: spiders.
    Cluster,
    /// None to see.
    Hidden,
}

/// A pattern painted over a species' colours. Colours are (r, g, b).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Marking {
    /// Round spots `size` blocks across, on about `density` of the places one could be.
    Spots { colour: [u8; 3], size: f32, density: f32 },
    /// Bands across the body, `width` wide every `spacing` blocks.
    Stripes { colour: [u8; 3], width: f32, spacing: f32 },
    /// The lower `height` (a fraction) of the legs.
    Socks { colour: [u8; 3], height: f32 },
    /// The last `length` (a fraction) of the tail.
    TailTip { colour: [u8; 3], length: f32 },
    /// Round the eyes and across the face.
    Mask { colour: [u8; 3] },
    /// Over the back.
    Saddle { colour: [u8; 3] },
    /// The muzzle and the lower face.
    Muzzle { colour: [u8; 3] },
    /// A stripe down the front of the face.
    Blaze { colour: [u8; 3] },
    /// The tips of the ears.
    EarTips { colour: [u8; 3] },
    /// A stripe along the spine.
    Dorsal { colour: [u8; 3], width: f32 },
    /// The whole head and neck: a drake's green head.
    Hood { colour: [u8; 3] },
    /// A ring round the neck, or round the body just behind the head.
    Collar { colour: [u8; 3], width: f32 },
}

/// How a species' skin is painted, beyond its colours: in `creatures.ron` under `body`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkinDef {
    pub eyes: Eyes,
    /// Eye radius in model blocks, scaled with the body; otherwise sized from the head.
    /// Explicit front eyes have a pale sclera around their iris.
    pub eye: Option<f32>,
    /// The eyes' colour; dark brown if not given.
    pub iris: Option<[u8; 3]>,
    /// How grainy the coat is, 0 (smooth, as a slime) to about 0.2 (shaggy).
    pub fur: f32,
    /// How high the second colour comes up the body's sides, as a fraction; 0 for none.
    pub belly: f32,
    /// The colour of hooves, paws or feet; the leg's own, darkened, if not given.
    pub feet: Option<[u8; 3]>,
    pub markings: Vec<Marking>,
}

impl Default for SkinDef {
    fn default() -> Self {
        Self { eyes: Eyes::Side, eye: None, iris: None, fur: 0.08, belly: 0.3, feet: None, markings: Vec::new() }
    }
}

type Rgb = Vec3;

fn rgb(c: [u8; 3]) -> Rgb {
    Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.0
}

/// A hash of a texel's place on the body, 0..1, for grain that stays put as it moves.
fn grain(seed: u64, at: Vec3, cell: f32, salt: u64) -> f32 {
    let q = (at * TEXELS / cell).floor();
    let key = mix(mix(mix(seed ^ salt, q.x as i64 as u64), q.y as i64 as u64), q.z as i64 as u64);
    unit_from(key)
}

/// What the painter knows of the body as a whole.
struct Body<'a> {
    def: &'a BodyDef,
    parts: &'a [Part],
    seed: u64,
    /// The head (or the body, for a creature with none), and its eyes' centres in its own
    /// frame, with their half size.
    head: Option<usize>,
    eyes: Vec<Vec3>,
    eye_size: f32,
    /// The tail's root and tip, to know how far along it a texel is.
    tail: Option<(Vec3, Vec3)>,
    /// The torso's centre, for patterns that run along the body.
    torso: Vec3,
}

impl Body<'_> {
    fn colour(&self, index: usize) -> Rgb {
        let colours = &self.def.colours;
        colours.get(index).or_else(|| colours.last()).map_or(Vec3::splat(0.5), |&c| rgb(c))
    }
}

/// What the painter knows of an individual's body.
fn body_of<'a>(def: &'a BodyDef, parts: &'a [Part], seed: u32) -> Body<'a> {
    let skin = &def.skin;
    let head = parts.iter().position(|p| p.role == Role::Head).or_else(|| (!parts.iter().any(|p| p.role == Role::Eye)).then_some(0));
    let (eyes, eye_size) = match head.map(|h| parts[h].size) {
        Some(size) if !parts.iter().any(|p| p.role == Role::Eye) => {
            let r = skin.eye.map_or_else(|| (size.y * 0.1).max(1.1 / TEXELS), |r| r * def.scale);
            let pairs: Vec<Vec3> = match skin.eyes {
                Eyes::Side => vec![Vec3::new(size.x / 2.0, size.y * 0.12, -size.z * 0.18)],
                Eyes::Front => vec![Vec3::new(size.x * 0.24, size.y * 0.1, -size.z / 2.0)],
                Eyes::Cluster => vec![
                    Vec3::new(size.x * 0.13, size.y * 0.16, -size.z / 2.0),
                    Vec3::new(size.x * 0.34, size.y * 0.04, -size.z / 2.0),
                    Vec3::new(size.x * 0.24, size.y * 0.34, -size.z / 2.0),
                ],
                Eyes::Hidden => Vec::new(),
            };
            let r = if skin.eyes == Eyes::Cluster { r * 0.7 } else { r };
            (pairs.into_iter().flat_map(|e| [e, e * Vec3::new(-1.0, 1.0, 1.0)]).collect(), r)
        }
        _ => (Vec::new(), 0.0),
    };
    let tail = parts.iter().find(|p| p.role == Role::Tail).map(|p| (p.at_rest(Vec3::NEG_Y * p.size.y / 2.0), p.at_rest(Vec3::Y * p.size.y / 2.0)));
    let torso = parts.iter().find(|p| p.role == Role::Torso).map_or(Vec3::ZERO, |p| p.centre);
    Body { def, parts, seed: mix(seed as u64, 0x5c1), head, eyes, eye_size, tail, torso }

}

/// Paints an individual's skin: `layout.width × layout.height` texels, RGBA, sRGB.
pub fn paint(def: &BodyDef, parts: &[Part], layout: &Layout, seed: u32) -> Vec<u8> {
    let body = body_of(def, parts, seed);
    let (width, height) = (layout.width as usize, layout.height as usize);
    let mut pixels = vec![0u8; width * height * 4];
    for (p, part) in parts.iter().enumerate() {
        for (f, face) in FACES.iter().enumerate() {
            let rect = layout.faces[p][f];
            let g = GUTTER as i32;
            for j in -g..rect.h as i32 + g {
                for i in -g..rect.w as i32 + g {
                    // Gutter texels repeat the patch's edge.
                    let (ci, cj) = (i.clamp(0, rect.w as i32 - 1), j.clamp(0, rect.h as i32 - 1));
                    let colour = texel(&body, p, part, f, face, rect, ci as u32, cj as u32);
                    let (x, y) = ((rect.x as i32 + i) as usize, (rect.y as i32 + j) as usize);
                    let at = (y * width + x) * 4;
                    let c = colour.clamp(Vec3::ZERO, Vec3::ONE) * 255.0;
                    pixels[at..at + 4].copy_from_slice(&[c.x.round() as u8, c.y.round() as u8, c.z.round() as u8, 255]);
                }
            }
        }
    }
    pixels
}

/// The colour of one texel.
#[allow(clippy::too_many_arguments)]
fn texel(body: &Body, index: usize, part: &Part, f: usize, face: &Face, rect: Rect, i: u32, j: u32) -> Rgb {
    let (u, v) = ((i as f32 + 0.5) / rect.w as f32, (j as f32 + 0.5) / rect.h as f32);
    // Where the texel lies in the box's own frame.
    let local = (face.normal * 0.5 + face.u * (u - 0.5) + face.v * (v - 0.5)) * part.size;
    let rim = i == 0 || j == 0 || i + 1 == rect.w || j + 1 == rect.h;
    paint_at(body, index, part, f, local, rim, true)
}

/// Paints an individual's skin through a volume: `dims` texels of [`TEXELS`] a block from
/// `origin`, RGBA, sRGB, x fastest. Each texel within `shell` blocks of the surface takes the
/// colour of the part `nearest` names (with its distance), as if on that part's nearest
/// face, so the painting runs across a smooth body as it does over boxes; the rest are left
/// clear.
pub fn paint_volume(def: &BodyDef, parts: &[Part], seed: u32, origin: Vec3, dims: [u32; 3], shell: f32, nearest: impl Fn(Vec3) -> Option<(usize, f32)>) -> Vec<u8> {
    let body = body_of(def, parts, seed);
    let [w, h, d] = dims.map(|n| n as usize);
    let mut pixels = vec![0u8; w * h * d * 4];
    for z in 0..d {
        for y in 0..h {
            for x in 0..w {
                let at = origin + (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / TEXELS;
                let Some((index, distance)) = nearest(at) else { continue };
                if distance.abs() > shell {
                    continue;
                }
                let part = &parts[index];
                // In the part's frame, and the face it is nearest, by its own proportions.
                let local = part.rest.inverse() * (at - part.centre);
                let across = (local / (part.size * 0.5).max(Vec3::splat(1e-4))).abs();
                let f = if across.x >= across.y && across.x >= across.z {
                    if local.x > 0.0 { 2 } else { 3 }
                } else if across.y >= across.z {
                    if local.y > 0.0 { 0 } else { 1 }
                } else if local.z > 0.0 {
                    4
                } else {
                    FRONT
                };
                // Onto the face, so its features (eyes, nostrils) land where they would on
                // the box.
                let n = FACES[f].normal;
                let on = local - n * local.dot(n) + n * (part.size * 0.5).dot(n.abs());
                // Eyes and nostrils only where the surface is near the face they sit on: on a
                // rounded head the sides fall away behind the face, and painting through them
                // would smear an eye across the cheek.
                let half = (part.size * 0.5).dot(n.abs());
                let features = local.dot(n) > half * 0.72;
                let colour = paint_at(&body, index, part, f, on, false, features).clamp(Vec3::ZERO, Vec3::ONE) * 255.0;
                let i = ((z * h + y) * w + x) * 4;
                pixels[i..i + 4].copy_from_slice(&[colour.x.round() as u8, colour.y.round() as u8, colour.z.round() as u8, 255]);
            }
        }
    }
    pixels
}

/// The colour at `local`, a point in part `index`'s own frame on (or near) its face `f`;
/// `rim` darkens it as a face's edge.
fn paint_at(body: &Body, index: usize, part: &Part, f: usize, local: Vec3, rim: bool, features: bool) -> Rgb {
    let skin = &body.def.skin;
    let face = &FACES[f];
    let at = part.at_rest(local);
    // How far up the box it is (0 at its bottom, 1 at its top), and along it.
    let up = (local.y / part.size.y + 0.5).clamp(0.0, 1.0);
    let normal = part.rest * face.normal;
    let seed = body.seed;

    let coat = matches!(part.role, Role::Torso | Role::Hump | Role::Neck | Role::Head | Role::Tail | Role::Thigh | Role::Ear);
    let mut colour = match part.role {
        Role::Eye => return eye_colour(body.def, Vec3::ZERO),
        Role::Mote => return body.colour(0) * 1.1,
        _ => body.colour(part.colour as usize),
    };

    // The belly, throat and underside in the second colour, its edge ragged.
    if skin.belly > 0.0 && coat && part.colour == 0 {
        let ragged = (grain(seed, at.with_y(0.0), 2.0, 11) - 0.5) * 0.12;
        let under = match part.role {
            Role::Torso | Role::Hump => normal.y < -0.5 || (part.role == Role::Torso && up < skin.belly + ragged),
            Role::Head => normal.y < -0.5 || (f == FRONT && up < 0.22 + ragged),
            Role::Neck => normal.y < -0.5,
            Role::Thigh => up < skin.belly * 0.8 + ragged,
            _ => false,
        };
        if under {
            colour = colour.lerp(body.colour(1), 0.85);
        }
    }

    // Hooves, paws and feet.
    if part.role == Role::Leg && part.size.y > part.size.x * 1.5 {
        let foot = (2.0 / (part.size.y * TEXELS)).max(0.14);
        let bottom = (local.y / part.size.y + 0.5) < foot || normal.y < -0.5;
        if bottom {
            colour = skin.feet.map_or(colour * 0.42, rgb);
        }
    }

    // The pale inside of an ear.
    if part.role == Role::Ear && f == FRONT {
        let inset_u = local.x.abs() < part.size.x * 0.5 - 1.0 / TEXELS || part.size.x * TEXELS < 3.0;
        if inset_u && up < 0.85 && up > 0.06 {
            // Pink on prey; hunters' are furred, a shade of their second colour.
            colour = if skin.eyes == Eyes::Side { body.colour(1).lerp(Vec3::new(0.86, 0.6, 0.6), 0.45) } else { body.colour(1) * 0.8 };
        }
    }

    for marking in &skin.markings {
        if let Some(c) = marking_at(body, marking, part, local, at, up, normal, f) {
            colour = rgb(c).lerp(colour, (grain(seed, at, 1.0, 23) - 0.5).abs() * 0.3);
        }
    }

    // Feathers in rows, rings on a shell.
    match part.role {
        Role::Wing => {
            let row = ((local.z / part.size.z + 0.5) * part.size.z * TEXELS / 3.0).fract();
            if row < 0.34 {
                colour *= 0.82;
            }
            // Flight feathers toward the tip, in the second colour.
            if local.z / part.size.z > 0.1 {
                colour = colour.lerp(body.colour(1), 0.65);
            }
        }
        Role::Tail if body.def.skin.fur < 0.05 && part.colour == 1 => {
            let row = (local.y.abs() * TEXELS / 3.0).fract();
            if row < 0.34 {
                colour *= 0.85;
            }
        }
        Role::Carapace => {
            // Plates in staggered rows, each with a dark seam and a lighter middle.
            let plate = 0.12;
            let row = (at.z / plate).floor();
            let across = at.x / plate + if row as i64 % 2 == 0 { 0.0 } else { 0.5 };
            let (fx, fz) = (across.rem_euclid(1.0), (at.z / plate).rem_euclid(1.0));
            let seam = 1.0 / (plate * TEXELS);
            if fx < seam || fz < seam || at.y > part.centre.y + part.size.y * 0.45 && normal.y > 0.5 && fx.min(fz) < seam * 1.5 {
                colour *= 0.55;
            } else if (fx - 0.5).abs() < 0.22 && (fz - 0.5).abs() < 0.22 {
                colour = colour.lerp(body.colour(2), 0.35);
            }
        }
        Role::Shell => {
            let side = Vec3::new(0.0, local.y, local.z).length() / part.size.y.max(1e-3);
            let ring = (side * 5.0 + grain(seed, at, 4.0, 31) * 0.3).fract();
            colour = if ring < 0.35 { colour * 0.7 } else { colour.lerp(body.colour(0), 0.15) };
        }
        _ => {}
    }

    // Eyes, nostrils, beak tips.
    if Some(index) == body.head && features {
        for &eye in &body.eyes {
            let d = (local - eye).abs();
            // Measured across the face the eye sits on, by the larger of the other two.
            let across = if eye.x.abs() >= part.size.x / 2.0 - 1e-4 { d.y.max(d.z) } else { d.x.max(d.y) };
            let depth = if eye.x.abs() >= part.size.x / 2.0 - 1e-4 { d.x } else { d.z };
            if across <= body.eye_size && depth < 1.0 / TEXELS {
                // The iris, a pupil (to the front on a side eye, in the middle of a front
                // one) and a catchlight in the top corner away from it.
                let high = local.y > eye.y + body.eye_size * 0.35;
                let (pupil, glint) = if skin.eyes == Eyes::Side {
                    (local.z < eye.z && !high, local.z > eye.z && high)
                } else {
                    let outward = (local.x - eye.x) * eye.x.signum();
                    ((local.x - eye.x).abs() < body.eye_size * 0.45 && !high, outward > body.eye_size * 0.3 && high)
                };
                let sclera = skin.eyes == Eyes::Front && skin.eye.is_some() && d.x > body.eye_size * 0.65;
                colour = if glint { Vec3::splat(0.95) } else if sclera { Vec3::splat(0.88) } else if pupil { Vec3::splat(0.03) } else { eye_colour(body.def, colour) };
                if body.eye_size * TEXELS < 1.6 && skin.eye.is_none() {
                    colour = Vec3::splat(0.04);
                }
            }
        }
        if skin.eyes == Eyes::Front && !body.parts.iter().any(|p| matches!(p.role, Role::Snout | Role::Beak)) && f == FRONT {
            // A small nose under the eyes.
            if local.x.abs() < part.size.x * 0.1 && (local.y + part.size.y * 0.12).abs() < part.size.y * 0.07 {
                colour = Vec3::splat(0.08);
            }
        }
    }
    if part.role == Role::Snout && f == FRONT && features {
        // A hunter's nose is a dark pad on the tip; a grazer's, two nostrils near the
        // top, or one small pink nose on a tiny muzzle.
        let wide = part.size.x * TEXELS;
        let x = local.x.abs() / part.size.x;
        let y = local.y / part.size.y;
        if skin.eyes != Eyes::Side {
            if x < 0.26 && y > 0.04 {
                colour = Vec3::new(0.07, 0.06, 0.06);
            }
        } else if wide >= 5.0 {
            if (0.14..0.34).contains(&x) && (0.08..0.34).contains(&y) {
                colour *= 0.4;
            }
        } else if local.x.abs() < 1.0 / TEXELS && (0.0..0.45).contains(&y) {
            colour = Vec3::new(0.78, 0.5, 0.5);
        }
        if local.y < -part.size.y * 0.32 && skin.eyes == Eyes::Front {
            colour *= 0.45;
        }
    }
    if part.role == Role::Beak && local.z < -part.size.z * 0.25 {
        colour *= 0.78;
    }

    // Grain: fine and coarse, so no coat is flat.
    if !matches!(part.role, Role::Horn | Role::Beak) || skin.fur > 0.0 {
        let fine = grain(seed, at, 1.0, 3) - 0.5;
        let coarse = grain(seed, at, 3.0, 5) - 0.5;
        let streak = grain(seed, at.with_y(0.0), 1.0, 7) - 0.5;
        colour *= 1.0 + skin.fur * (fine * 1.0 + coarse * 0.8 + streak * 0.6);
    }
    // Lighter on top, darker underneath, as fur and the sky make it.
    if coat {
        colour *= 0.9 + 0.18 * up;
    }
    if rim {
        colour *= 0.88;
    }
    colour
}

fn eye_colour(def: &BodyDef, _behind: Rgb) -> Rgb {
    match def.skin.iris {
        Some(c) => rgb(c),
        None if def.glows => Vec3::new(0.95, 0.98, 1.0),
        None => Vec3::new(0.32, 0.19, 0.09),
    }
}

/// The colour a marking gives a texel, if it covers it.
#[allow(clippy::too_many_arguments)]
fn marking_at(body: &Body, marking: &Marking, part: &Part, local: Vec3, at: Vec3, up: f32, normal: Vec3, f: usize) -> Option<[u8; 3]> {
    let coat = matches!(part.role, Role::Torso | Role::Hump | Role::Neck | Role::Thigh | Role::Head | Role::Tail) && part.colour == 0;
    let back = matches!(part.role, Role::Torso | Role::Hump | Role::Neck);
    match *marking {
        Marking::Spots { colour, size, density } => {
            if !coat || (part.role == Role::Torso && up < body.def.skin.belly) {
                return None;
            }
            let cell = (size * 2.2).max(2.0 / TEXELS);
            let base = (at / cell).floor();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let c = base + Vec3::new(dx as f32, dy as f32, dz as f32);
                        let key = mix(mix(mix(body.seed ^ 41, c.x as i64 as u64), c.y as i64 as u64), c.z as i64 as u64);
                        if unit_from(key) > density {
                            continue;
                        }
                        let jitter = Vec3::new(unit_from(mix(key, 1)), unit_from(mix(key, 2)), unit_from(mix(key, 3)));
                        let centre = (c + jitter) * cell;
                        let radius = size / 2.0 * (0.7 + 0.6 * unit_from(mix(key, 4)));
                        if at.distance(centre) < radius {
                            return Some(colour);
                        }
                    }
                }
            }
            None
        }
        Marking::Stripes { colour, width, spacing } => {
            let striped = coat || part.role == Role::Leg && up > 0.4;
            if !striped || (part.role == Role::Torso && up < body.def.skin.belly) || normal.y < -0.5 {
                return None;
            }
            let wobble = ((at.y * 9.0 + at.x * 3.0).sin() + (grain(body.seed, at, 4.0, 43) - 0.5)) * spacing * 0.14;
            let along = (at.z - body.torso.z + wobble).rem_euclid(spacing);
            (along < width).then_some(colour)
        }
        Marking::Socks { colour, height } => (part.role == Role::Leg && up < height).then_some(colour),
        Marking::TailTip { colour, length } => {
            let (root, tip) = body.tail?;
            let along = (at - root).dot(tip - root) / (tip - root).length_squared().max(1e-6);
            (part.role == Role::Tail && along > 1.0 - length).then_some(colour)
        }
        Marking::Mask { colour } => {
            let head = body.parts.get(body.head?)?;
            if part.role != Role::Head {
                return None;
            }
            let eye_line = (local.y - head.size.y * 0.1).abs() < head.size.y * 0.2;
            let forward = local.z < -head.size.z * 0.05;
            (eye_line && forward).then_some(colour)
        }
        Marking::Saddle { colour } => {
            let over = normal.y > 0.5 || up > 0.62;
            let middle = (at.z - body.torso.z).abs() < part.size.z * 0.36;
            (back && part.role != Role::Neck && over && middle).then_some(colour)
        }
        Marking::Muzzle { colour } => match part.role {
            Role::Snout => Some(colour),
            Role::Head if f == FRONT && up < 0.45 => Some(colour),
            Role::Head if normal.y < -0.5 => Some(colour),
            _ => None,
        },
        Marking::Blaze { colour } => {
            let on = matches!(part.role, Role::Head | Role::Snout) && (f == FRONT || normal.y > 0.5) && local.x.abs() < part.size.x * 0.13;
            on.then_some(colour)
        }
        Marking::EarTips { colour } => (part.role == Role::Ear && up > 0.62).then_some(colour),
        Marking::Hood { colour } => matches!(part.role, Role::Head | Role::Neck).then_some(colour),
        Marking::Collar { colour, width } => {
            let head = body.parts.get(body.head?)?;
            let behind = at.z - (head.centre.z + head.size.z / 2.0);
            let on = matches!(part.role, Role::Torso | Role::Neck) && behind > 0.0 && behind < width;
            on.then_some(colour)
        }
        Marking::Dorsal { colour, width } => {
            let on = (back || part.role == Role::Tail) && (normal.y > 0.5 || up > 0.9) && at.x.abs() < width / 2.0;
            on.then_some(colour)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature::model::{HornShape, Model};

    fn rabbit() -> BodyDef {
        BodyDef {
            size: (0.4, 0.45),
            model: Model::Quadruped {
                body: (0.3, 0.26, 0.42),
                head: (0.24, 0.22, 0.24),
                legs: 0.1,
                ears: Some((0.07, 0.2)),
                tail: Some((0.1, 0.1, 0.08)),
                horns: None,
                horn_shape: HornShape::Curved,
                snout: None,
                neck: None,
                haunches: true,
                hump: None,
                shell: None,
                tail_droop: -30.0,
                ear_lean: 10.0,
            },
            colours: vec![[150, 118, 86], [226, 216, 196], [70, 52, 40]],
            glows: false,
            scale: 1.0,
            skin: SkinDef { markings: vec![Marking::Spots { colour: [40, 30, 20], size: 0.08, density: 0.5 }], ..Default::default() },
        }
    }

    #[test]
    fn patches_fit_and_never_overlap() {
        let def = rabbit();
        let parts = def.model.parts(1.0);
        let layout = layout(&parts);
        let mut taken = vec![false; (layout.width * layout.height) as usize];
        for rects in &layout.faces {
            for r in rects {
                assert!(r.x >= GUTTER && r.y >= GUTTER && r.x + r.w + GUTTER <= layout.width && r.y + r.h + GUTTER <= layout.height, "{r:?} outside");
                for y in r.y - GUTTER..r.y + r.h + GUTTER {
                    for x in r.x - GUTTER..r.x + r.w + GUTTER {
                        let at = (y * layout.width + x) as usize;
                        assert!(!taken[at], "{r:?} overlaps another patch");
                        taken[at] = true;
                    }
                }
            }
        }
    }

    #[test]
    fn explicit_front_eyes_have_sclera_iris_and_pupils_on_a_small_human_head() {
        let mut def: BodyDef = ron::from_str(
            r#"(
            size: (0.6, 1.8),
            model: Biped(body: (0.5, 0.7, 0.3), head: (0.42, 0.42, 0.42), legs: 0.72, arms: 0.66),
            colours: [(180, 130, 100)],
            skin: (eyes: Front, eye: 0.065, iris: (60, 100, 150), fur: 0.0, belly: 0.0),
        )"#,
        ).unwrap();
        let parts = def.model.parts(def.scale);
        let layout = layout(&parts);
        let body = body_of(&def, &parts, 7);
        let head = body.head.unwrap();
        let part = &parts[head];
        let rect = layout.faces[head][FRONT];
        let pixels = paint(&def, &parts, &layout, 7);
        assert_eq!(pixels, paint(&def, &parts, &layout, 7));
        for eye in &body.eyes {
            let mut colours = [0; 4];
            for j in 0..rect.h {
                for i in 0..rect.w {
                    let local = Vec3::new(
                        -((i as f32 + 0.5) / rect.w as f32 - 0.5) * part.size.x,
                        -((j as f32 + 0.5) / rect.h as f32 - 0.5) * part.size.y,
                        -part.size.z * 0.5,
                    );
                    if (local - *eye).abs().max_element() > body.eye_size {
                        continue;
                    }
                    let at = (((rect.y + j) * layout.width + rect.x + i) * 4) as usize;
                    let c = &pixels[at..at + 3];
                    colours[0] += usize::from(c.iter().all(|&v| v > 200) && c[0] < 235);
                    colours[1] += usize::from(c[2] > c[1] && c[1] > c[0]);
                    colours[2] += usize::from(c.iter().all(|&v| v < 20));
                    colours[3] += usize::from(c.iter().all(|&v| v >= 235));
                }
            }
            assert!(colours.iter().all(|&n| n > 0), "missing sclera, iris, pupil or catchlight: {colours:?}");
        }
        def.skin.eye = None;
        assert_ne!(pixels, paint(&def, &parts, &layout, 7), "the old black-eye painting should fail the comparison");
        def.skin.eye = Some(0.065);
        def.scale = 0.5;
        let small = def.model.parts(def.scale);
        assert_eq!(body_of(&def, &small, 7).eye_size, 0.065 * 0.5);
    }

    #[test]
    fn individuals_differ_and_heads_have_eyes() {
        let def = rabbit();
        let parts = def.model.parts(1.0);
        let layout = layout(&parts);
        let one = paint(&def, &parts, &layout, 1);
        assert_eq!(one, paint(&def, &parts, &layout, 1), "the same seed paints the same skin");
        assert_ne!(one, paint(&def, &parts, &layout, 2), "another seed paints another");
        // A near-black pupil somewhere on the head's sides.
        let head = parts.iter().position(|p| p.role == Role::Head).unwrap();
        let dark = [2, 3].iter().any(|&f| {
            let r = layout.faces[head][f];
            (r.y..r.y + r.h).any(|y| (r.x..r.x + r.w).any(|x| one[((y * layout.width + x) * 4) as usize] < 20))
        });
        assert!(dark, "no eye on the head's sides");
    }
}
