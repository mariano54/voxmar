//! Block shapes: blocks drawn as boxes rather than as a whole cube (a barrel, a crate, a
//! slab, stairs, a door), as Minecraft's block models are. A package's `shapes.ron` names
//! each shape's boxes, in 16ths of a block; a block takes one with `render: Model` and
//! `shape: "<name>"`. The boxes here, from the package that defines the block, are the
//! block's true shape (what it hides, and later what it collides with); the client draws
//! them, and a texture pack may give other boxes to draw, never other collision.
//!
//! ```text
//! (
//!     limits: (boxes: 32, quads: 4096),
//!     shapes: {
//!         "slab": (boxes: [(from: (0, 0, 0), to: (16, 8, 16), cull: true)]),
//!         "barrel": (boxes: [
//!             (from: (2, 0, 4), to: (14, 16, 12), textures: Sides(top: "barrel_top", bottom: "barrel_top", side: "barrel_side")),
//!             (from: (2, 0, 4), to: (14, 16, 12), turn: 45.0, ...),
//!         ]),
//!     },
//! )
//! ```
//!
//! A box without `textures` takes its block's. `turn` turns a box about the cell's
//! vertical axis, in degrees; a turned box hides nothing and, for collision and
//! selection, counts as its unturned bound. `cull` lets the box's faces on the cell's
//! boundary hide behind a solid neighbour.

use std::collections::BTreeMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::block::{Facing, RegistryError, Textures};
use crate::coords::Face;

/// How many boxes a shape may have, and how many quads all shapes together, as the
/// renderer's table holds them. Single-valued data a package sets field by field (Design
/// draft 91): core sets them, a later package may change them. None set, no shape loads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeLimits {
    pub boxes: usize,
    pub quads: usize,
}

crate::content::settable! {
    /// What one package's `shapes.ron` sets of the shape limits.
    SetShapeLimits for ShapeLimits { boxes: usize, quads: usize }
}

/// The most quads all shapes may have together, whatever the limits say: a quad's number in
/// the renderer's table takes 14 bits of a vertex (mesh.rs `emit_model`). Each facing of a
/// shape is its own quads, so a piece of six boxes in four facings takes 144.
pub const MAX_SHAPE_QUADS: usize = 1 << 14;

/// A package's `shapes.ron`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShapeFile {
    /// The limits every shape keeps to, as this package sets them.
    pub limits: Option<SetShapeLimits>,
    pub shapes: BTreeMap<String, ShapeDef>,
}

impl ShapeFile {
    pub fn read(namespace: &str, source: &str) -> Result<Self, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/shapes.ron: {e}")))
    }
}

/// A shape as authored: its boxes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShapeDef {
    pub boxes: Vec<ShapeBox>,
    /// What a creature's blow meets of it at a player standing in the open part of its
    /// cell, gaps and all (`Shape::hull`): a railing's balusters are no way through. None,
    /// the span of all its boxes, which suits a shape of one run (a railing, a fence); a
    /// shape of two (a railing's corner) names one span for each.
    pub hull: Vec<Span>,
}

/// A span of a shape's cell, in 16ths of a block from its lower corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub from: [f32; 3],
    pub to: [f32; 3],
}

impl ShapeDef {
    /// The shape turned over, top to bottom (a top slab from a slab): each box mirrored
    /// about the cell's middle height, keeping its textures the right way up.
    pub fn upside_down(&self) -> ShapeDef {
        let boxes = self
            .boxes
            .iter()
            .map(|b| ShapeBox { from: [b.from[0], 16.0 - b.to[1], b.from[2]], to: [b.to[0], 16.0 - b.from[1], b.to[2]], ..b.clone() })
            .collect();
        let hull = self.hull.iter().map(|h| Span { from: [h.from[0], 16.0 - h.to[1], h.from[2]], to: [h.to[0], 16.0 - h.from[1], h.to[2]] }).collect();
        ShapeDef { boxes, hull }
    }
}

/// One box of a shape, in 16ths of a block from the cell's lower corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShapeBox {
    pub from: [f32; 3],
    pub to: [f32; 3],
    /// Degrees about the cell's vertical axis (through its middle).
    pub turn: f32,
    /// Its own textures; none takes the block's.
    pub textures: Option<Textures>,
    /// Its faces on the cell's boundary hide behind a neighbour that hides faces.
    pub cull: bool,
}

impl Default for ShapeBox {
    fn default() -> Self {
        Self { from: [0.0; 3], to: [16.0; 3], turn: 0.0, textures: None, cull: false }
    }
}

/// Shapes by full name (`base:barrel`), and the limits they keep to.
#[derive(Clone, Debug, Default)]
pub struct Shapes {
    pub limits: ShapeLimits,
    pub defs: BTreeMap<String, ShapeDef>,
}

impl Shapes {
    /// Every package's shapes, names qualified by its namespace, under `limits` (as the
    /// packages set them). A shape is defined once: a package naming another's shape
    /// (`base:slab` from a mod) is refused, naming both.
    pub fn gather(files: Vec<(String, ShapeFile)>, limits: ShapeLimits) -> Result<Self, RegistryError> {
        if limits.quads > MAX_SHAPE_QUADS {
            return Err(RegistryError(format!(
                "shapes.ron: shapes may have at most {MAX_SHAPE_QUADS} quads in all, which is as many as a vertex can number, not {}",
                limits.quads
            )));
        }
        let mut shapes = Shapes { limits, defs: BTreeMap::new() };
        let mut by = BTreeMap::new();
        for (namespace, file) in files {
            for (name, def) in file.shapes {
                let name = if name.contains(':') { name } else { format!("{namespace}:{name}") };
                if let Some(earlier) = by.insert(name.clone(), namespace.clone()) {
                    return Err(RegistryError(format!("{namespace}/shapes.ron: shape `{name}` is already defined by {earlier}")));
                }
                shapes.defs.insert(name, def);
            }
        }
        Ok(shapes)
    }
}

/// One quad of a compiled shape, in blocks from the cell's lower corner: its corners
/// counter-clockwise seen from outside, their texture coordinates (0..1 across the
/// texture), its outward normal and the cube face it is shaded and lit as.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeQuad {
    pub corners: [Vec3; 4],
    pub uvs: [[f32; 2]; 4],
    pub normal: Vec3,
    pub face: Face,
    /// The face of the cell it lies on, if it lies on one: it takes the light of the cell
    /// beyond as well as its own, as a cube face does.
    pub boundary: Option<Face>,
    /// Whether it hides behind a neighbour across `boundary` that hides faces (`cull`).
    pub cull: bool,
}

/// A shape made ready to draw in one facing. Blocks that take the same shape in the same
/// facing share it; each names its own textures for the quads (see [`compile`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub quads: Vec<ShapeQuad>,
    /// Which of the cell's faces its boxes cover whole (by `Face`): a covered face hides
    /// the neighbour's face against it, and a shape covering all six is as a cube to light.
    pub full: [bool; 6],
    /// Its boxes as collision and selection see them: unturned bounds, in blocks.
    pub bounds: Vec<(Vec3, Vec3)>,
    /// What a blow at a player in its cell meets (`ShapeDef::hull`), in blocks, turned as
    /// `bounds` are: the span of `bounds` unless the shape names its own.
    pub hull: Vec<(Vec3, Vec3)>,
}

impl Shape {
    pub fn is_full_cube(&self) -> bool {
        self.full.iter().all(|&f| f)
    }
}

/// Makes `def` (named `name`, for block `block`, whose own textures are `textures`) ready to
/// draw facing `facing`, checking it as it goes; with the name of each quad's texture, from
/// its box or the block.
pub fn compile(
    name: &str,
    block: &str,
    def: &ShapeDef,
    textures: &Textures,
    facing: Facing,
    limits: ShapeLimits,
) -> Result<(Shape, Vec<String>), RegistryError> {
    let error = |message: String| RegistryError(format!("block `{block}`, shape `{name}`: {message}"));
    if def.boxes.is_empty() {
        return Err(error("it has no boxes".into()));
    }
    if def.boxes.len() > limits.boxes {
        return Err(error(format!("it has {} boxes, more than the {} a shape may have", def.boxes.len(), limits.boxes)));
    }
    if def.hull.len() > limits.boxes {
        return Err(error(format!("its hull has {} spans, more than the {} boxes a shape may have", def.hull.len(), limits.boxes)));
    }
    let quarter_turn = |p: Vec3| {
        let (x, z) = (p.x - 0.5, p.z - 0.5);
        let (x, z) = match facing {
            Facing::North => (x, z),
            Facing::East => (-z, x),
            Facing::South => (-x, -z),
            Facing::West => (z, -x),
        };
        Vec3::new(x + 0.5, p.y, z + 0.5)
    };
    let mut shape = Shape::default();
    let mut names = Vec::new();
    for (i, b) in def.boxes.iter().enumerate() {
        let inside = (0..3).all(|k| (0.0..=16.0).contains(&b.from[k]) && (0.0..=16.0).contains(&b.to[k]) && b.from[k] < b.to[k]);
        if !inside {
            return Err(error(format!("box {i} must run from less to more on every axis within 0..16: from {:?} to {:?}", b.from, b.to)));
        }
        let turned = b.turn.rem_euclid(360.0) != 0.0;
        if turned && b.cull {
            return Err(error(format!("box {i} is turned, so it cannot hide behind a neighbour (cull)")));
        }
        let on_boundary = (0..3).any(|k| b.from[k] == 0.0 || b.to[k] == 16.0);
        if b.cull && !on_boundary {
            return Err(error(format!("box {i} hides behind neighbours (cull) but has no face on the cell's boundary")));
        }
        let from = Vec3::from(b.from) / 16.0;
        let to = Vec3::from(b.to) / 16.0;
        let box_textures = b.textures.as_ref().unwrap_or(textures);
        // About the cell's middle: the box's own turn, then the block's facing (north
        // as authored; a quarter turn clockwise seen from above for each step round).
        let quarter = match facing {
            Facing::North => 0.0,
            Facing::East => -90.0,
            Facing::South => 180.0,
            Facing::West => 90.0,
        };
        let angle = (b.turn + quarter).to_radians();
        let turn = |p: Vec3| {
            let (s, c) = angle.sin_cos();
            let (x, z) = (p.x - 0.5, p.z - 0.5);
            Vec3::new(x * c + z * s + 0.5, p.y, -x * s + z * c + 0.5)
        };
        for face in Face::ALL {
            let Some(texture) = box_textures.face(face) else { continue };
            let (corners, uvs) = box_face(from, to, face);
            let normal = turn(face.normal().as_vec3() + Vec3::splat(0.5)) - Vec3::splat(0.5);
            let corners = corners.map(turn);
            let shaded = nearest_face(normal);
            let boundary = on_boundary_face(&corners, shaded);
            names.push(texture.to_owned());
            shape.quads.push(ShapeQuad {
                corners,
                uvs,
                normal,
                face: shaded,
                boundary,
                cull: b.cull && boundary.is_some(),
            });
        }
        // Collision and selection: the unturned box, turned only by the facing, which is a
        // whole number of quarter turns and so exact.
        let (a, c) = (quarter_turn(from), quarter_turn(to));
        shape.bounds.push((a.min(c), a.max(c)));
    }
    shape.full = full_faces(&shape.bounds, def);
    for (i, h) in def.hull.iter().enumerate() {
        if !(0..3).all(|k| (0.0..=16.0).contains(&h.from[k]) && (0.0..=16.0).contains(&h.to[k]) && h.from[k] < h.to[k]) {
            return Err(error(format!("hull span {i} must run from less to more on every axis within 0..16: from {:?} to {:?}", h.from, h.to)));
        }
        let (a, c) = (quarter_turn(Vec3::from(h.from) / 16.0), quarter_turn(Vec3::from(h.to) / 16.0));
        shape.hull.push((a.min(c), a.max(c)));
    }
    if shape.hull.is_empty() {
        let span = shape.bounds.iter().fold(None::<(Vec3, Vec3)>, |span, &(a, c)| Some(span.map_or((a, c), |(min, max)| (min.min(a), max.max(c)))));
        shape.hull.extend(span);
    }
    Ok((shape, names))
}

/// Which of the cell's faces the shape's unturned boxes cover whole, sampled at the
/// middle of each 16th.
fn full_faces(bounds: &[(Vec3, Vec3)], def: &ShapeDef) -> [bool; 6] {
    let flat: Vec<&(Vec3, Vec3)> = bounds.iter().zip(&def.boxes).filter(|(_, b)| b.turn.rem_euclid(360.0) == 0.0).map(|(bound, _)| bound).collect();
    let mut full = [false; 6];
    for face in Face::ALL {
        let axis = match face {
            Face::PosX | Face::NegX => 0,
            Face::PosY | Face::NegY => 1,
            Face::PosZ | Face::NegZ => 2,
        };
        let outer = face.normal()[axis] > 0;
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        full[face as usize] = (0..16).all(|i| {
            (0..16).all(|j| {
                let (pu, pv) = ((i as f32 + 0.5) / 16.0, (j as f32 + 0.5) / 16.0);
                flat.iter().any(|(lo, hi)| {
                    let touches = if outer { hi[axis] >= 1.0 } else { lo[axis] <= 0.0 };
                    touches && lo[u] <= pu && pu <= hi[u] && lo[v] <= pv && pv <= hi[v]
                })
            })
        });
    }
    full
}

/// The face of the cell a quad facing `face` lies on, if all its corners are on it.
fn on_boundary_face(corners: &[Vec3; 4], face: Face) -> Option<Face> {
    let normal = face.normal();
    let axis = (0..3).find(|&k| normal[k] != 0)?;
    let plane = if normal[axis] > 0 { 1.0 } else { 0.0 };
    corners.iter().all(|c| (c[axis] - plane).abs() < 1e-5).then_some(face)
}

/// The cube face whose normal is nearest `normal`.
fn nearest_face(normal: Vec3) -> Face {
    Face::ALL.into_iter().max_by(|a, b| normal.dot(a.normal().as_vec3()).total_cmp(&normal.dot(b.normal().as_vec3()))).unwrap_or(Face::PosY)
}

/// A box face's corners, counter-clockwise from outside, and their texture coordinates as
/// Minecraft lays a texture on a block model by default: the part of the texture under the
/// face's position on the cell (tops and bottoms by x and z, sides by their run and height,
/// upright).
fn box_face(from: Vec3, to: Vec3, face: Face) -> ([Vec3; 4], [[f32; 2]; 4]) {
    let (a, b) = (from, to);
    let corners = match face {
        Face::PosX => [Vec3::new(b.x, a.y, b.z), Vec3::new(b.x, a.y, a.z), Vec3::new(b.x, b.y, a.z), Vec3::new(b.x, b.y, b.z)],
        Face::NegX => [Vec3::new(a.x, a.y, a.z), Vec3::new(a.x, a.y, b.z), Vec3::new(a.x, b.y, b.z), Vec3::new(a.x, b.y, a.z)],
        Face::PosY => [Vec3::new(a.x, b.y, b.z), Vec3::new(b.x, b.y, b.z), Vec3::new(b.x, b.y, a.z), Vec3::new(a.x, b.y, a.z)],
        Face::NegY => [Vec3::new(a.x, a.y, a.z), Vec3::new(b.x, a.y, a.z), Vec3::new(b.x, a.y, b.z), Vec3::new(a.x, a.y, b.z)],
        Face::PosZ => [Vec3::new(a.x, a.y, b.z), Vec3::new(b.x, a.y, b.z), Vec3::new(b.x, b.y, b.z), Vec3::new(a.x, b.y, b.z)],
        Face::NegZ => [Vec3::new(b.x, a.y, a.z), Vec3::new(a.x, a.y, a.z), Vec3::new(a.x, b.y, a.z), Vec3::new(b.x, b.y, a.z)],
    };
    let uv = |p: Vec3| match face {
        Face::PosX => [1.0 - p.z, 1.0 - p.y],
        Face::NegX => [p.z, 1.0 - p.y],
        Face::PosZ => [p.x, 1.0 - p.y],
        Face::NegZ => [1.0 - p.x, 1.0 - p.y],
        Face::PosY => [p.x, p.z],
        Face::NegY => [p.x, 1.0 - p.z],
    };
    (corners, corners.map(uv))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: ShapeLimits = ShapeLimits { boxes: 8, quads: 4096 };

    fn shape(boxes: Vec<ShapeBox>) -> Result<Shape, RegistryError> {
        compile("test:shape", "test:block", &ShapeDef { boxes, ..Default::default() }, &Textures::All("stone".into()), Facing::North, LIMITS).map(|(shape, _)| shape)
    }

    #[test]
    fn a_slab_covers_its_bottom_and_no_side_whole() {
        let slab = shape(vec![ShapeBox { to: [16.0, 8.0, 16.0], cull: true, ..Default::default() }]).unwrap();
        assert_eq!(slab.full, [false, false, false, true, false, false]);
        assert!(!slab.is_full_cube());
        assert_eq!(slab.quads.len(), 6);
        // The top is half way up and does not hide behind a neighbour above; the sides do.
        let top = slab.quads.iter().find(|q| q.face == Face::PosY).unwrap();
        assert!(top.corners.iter().all(|c| c.y == 0.5) && top.boundary.is_none() && !top.cull);
        assert!(slab.quads.iter().filter(|q| q.cull).count() == 5);
    }

    #[test]
    fn two_boxes_can_make_a_whole_cube() {
        let cube = shape(vec![
            ShapeBox { to: [16.0, 8.0, 16.0], ..Default::default() },
            ShapeBox { from: [0.0, 8.0, 0.0], ..Default::default() },
        ])
        .unwrap();
        assert!(cube.is_full_cube());
    }

    #[test]
    fn bad_boxes_are_refused_by_name() {
        let refused = |b: ShapeBox| shape(vec![b]).unwrap_err().0;
        assert!(refused(ShapeBox { to: [16.0, 17.0, 16.0], ..Default::default() }).contains("block `test:block`, shape `test:shape`"));
        assert!(refused(ShapeBox { from: [4.0, 0.0, 0.0], to: [4.0, 16.0, 16.0], ..Default::default() }).contains("less to more"));
        assert!(refused(ShapeBox { from: [4.0; 3], to: [12.0; 3], cull: true, ..Default::default() }).contains("boundary"));
        assert!(refused(ShapeBox { turn: 45.0, cull: true, ..Default::default() }).contains("turned"));
        let many = (0..9).map(|_| ShapeBox::default()).collect();
        assert!(shape(many).unwrap_err().0.contains("more than the 8"));
    }

    /// A shape is defined once, and the limits never let more quads in than a vertex can
    /// number; a later package may change the limits field by field.
    #[test]
    fn shapes_are_gathered_once_within_what_a_vertex_can_number() {
        let file = |source: &str| ShapeFile::read("test", source).unwrap();
        let slab = r#"(shapes: {"slab": (boxes: [(to: (16, 8, 16))])})"#;
        let refused = Shapes::gather(vec![("base".into(), file(slab)), ("addon".into(), file(r#"(shapes: {"base:slab": (boxes: [(to: (16, 4, 16))])})"#))], LIMITS);
        assert!(refused.unwrap_err().0.contains("addon/shapes.ron: shape `base:slab` is already defined by base"));
        let too_many = ShapeLimits { quads: MAX_SHAPE_QUADS + 1, ..LIMITS };
        assert!(Shapes::gather(vec![("base".into(), file(slab))], too_many).unwrap_err().0.contains("at most 16384"));
        let set = file(r#"(limits: (quads: 64))"#).limits.unwrap();
        assert_eq!(crate::content::Settable::with(LIMITS, &set), (ShapeLimits { boxes: 8, quads: 64 }, vec!["quads"]));
    }

    #[test]
    fn a_turned_box_hides_nothing_and_collides_as_its_bound() {
        let post = shape(vec![ShapeBox { from: [4.0, 0.0, 4.0], to: [12.0, 16.0, 12.0], turn: 45.0, ..Default::default() }]).unwrap();
        assert_eq!(post.full, [false; 6]);
        assert_eq!(post.bounds, vec![(Vec3::new(0.25, 0.0, 0.25), Vec3::new(0.75, 1.0, 0.75))]);
        // Its sides run diagonally.
        let side = post.quads.iter().find(|q| q.normal.y == 0.0).unwrap();
        assert!((side.normal.x.abs() - side.normal.z.abs()).abs() < 1e-5);
    }

    #[test]
    fn facings_turn_the_boxes() {
        // A half-block against the north side (-z), turned to face east, lies against the east (+x).
        let def = ShapeDef { boxes: vec![ShapeBox { to: [16.0, 16.0, 8.0], ..Default::default() }], ..Default::default() };
        let (east, _) = compile("s", "b", &def, &Textures::All("stone".into()), Facing::East, LIMITS).unwrap();
        assert_eq!(east.bounds, vec![(Vec3::new(0.5, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0))]);
        assert!(east.full[Face::PosX as usize] && !east.full[Face::NegZ as usize]);
        let quads_east = east.quads.iter().filter(|q| q.face == Face::PosX).count();
        assert_eq!(quads_east, 1);
    }
}
