//! How the quest book draws its web (`quest_book.ron`): the shapes quests stand in and
//! which quest takes which, the colours of each standing, the lines between quests and the
//! light that runs along those into an open quest, and the dotted ground under them. Read
//! and checked at launch; a later package changes what it gives.
//!
//! A shape is drawn here, at the size the book's zoom gives it, a texel to every `PX`
//! screen pixels like the rest of the interface's pixel art: a face to tint by standing,
//! and a rim with a lit and a shaded side and a dark outline, tinted apart.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::content::ContentFile;
use voxmar_engine::quest::QuestDef;

use super::combat::Rgb;
use crate::textures::Rgba;

pub const FILE: &str = "quest_book.ron";

/// A shape a quest stands in: a regular polygon of `sides` (none, a circle) turned `turn`
/// degrees, its corners rounded by `round` of its radius, and with `teeth` a cog's teeth
/// that deep, one a side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shape {
    pub sides: u8,
    pub turn: f32,
    pub round: f32,
    pub teeth: f32,
}

/// The colours of a quest standing so: its face behind the icon, and its rim.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tones {
    pub face: Rgb,
    pub rim: Rgb,
}

/// The lines between quests: `width` interface pixels thick, `done` once the first is done
/// and `ahead` before; a light of `light` runs `flow` lengths of a line a second into an
/// open quest, lit over `glow` of the line.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lines {
    pub width: f32,
    pub done: Rgb,
    pub ahead: Rgb,
    pub flow: f32,
    pub glow: f32,
    pub light: Rgb,
}

/// The web's ground: a dot every `spacing` interface pixels.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub spacing: u32,
    pub dot: Rgb,
    pub alpha: f32,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LookFile {
    shapes: BTreeMap<String, Shape>,
    shape: Option<String>,
    start: Option<String>,
    hub: Option<String>,
    hub_after: Option<usize>,
    quests: BTreeMap<String, String>,
    waiting: Option<Tones>,
    open: Option<Tones>,
    done: Option<Tones>,
    ready: Option<Tones>,
    lines: Option<Lines>,
    grid: Option<Grid>,
}

/// How a quest stands, as the web draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    Waiting,
    Open,
    Done,
    /// Done, with its rewards waiting to be claimed.
    Ready,
}

/// How the quest book draws its web, from every package's `quest_book.ron`.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct QuestLook {
    pub shapes: BTreeMap<String, Shape>,
    pub shape: String,
    pub start: String,
    pub hub: String,
    pub hub_after: usize,
    pub quests: BTreeMap<String, String>,
    pub waiting: Tones,
    pub open: Tones,
    pub done: Tones,
    pub ready: Tones,
    pub lines: Lines,
    pub grid: Grid,
}

impl QuestLook {
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut all = LookFile::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let given: LookFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            all.shapes.extend(given.shapes);
            all.quests.extend(given.quests);
            all.shape = given.shape.or(all.shape);
            all.start = given.start.or(all.start);
            all.hub = given.hub.or(all.hub);
            all.hub_after = given.hub_after.or(all.hub_after);
            all.waiting = given.waiting.or(all.waiting);
            all.open = given.open.or(all.open);
            all.done = given.done.or(all.done);
            all.ready = given.ready.or(all.ready);
            all.lines = given.lines.or(all.lines);
            all.grid = given.grid.or(all.grid);
        }
        fn given<T>(value: Option<T>, name: &str) -> Result<T, String> {
            value.ok_or(format!("no package's {FILE} gives {name}"))
        }
        let look = QuestLook {
            shape: given(all.shape, "shape")?,
            start: given(all.start, "start")?,
            hub: given(all.hub, "hub")?,
            hub_after: given(all.hub_after, "hub_after")?,
            waiting: given(all.waiting, "waiting")?,
            open: given(all.open, "open")?,
            done: given(all.done, "done")?,
            ready: given(all.ready, "ready")?,
            lines: given(all.lines, "lines")?,
            grid: given(all.grid, "grid")?,
            shapes: all.shapes,
            quests: all.quests,
        };
        look.check()?;
        Ok(look)
    }

    fn check(&self) -> Result<(), String> {
        for (name, shape) in &self.shapes {
            if shape.sides != 0 && shape.sides < 3 {
                return Err(format!("{FILE}: the shape {name} has {} sides; a shape has none (a circle) or 3 or more", shape.sides));
            }
            if !(0.0..=0.5).contains(&shape.round) || !(0.0..=0.4).contains(&shape.teeth) || !shape.turn.is_finite() {
                return Err(format!("{FILE}: the shape {name} rounds {} (0 to 0.5), has teeth {} (0 to 0.4) and turns {}", shape.round, shape.teeth, shape.turn));
            }
            if shape.teeth > 0.0 && shape.sides < 3 {
                return Err(format!("{FILE}: the shape {name} has teeth but no sides to set them on"));
            }
        }
        for (what, name) in [("shape", &self.shape), ("start", &self.start), ("hub", &self.hub)].into_iter().chain(self.quests.iter().map(|(quest, shape)| (quest.as_str(), shape))) {
            if !self.shapes.contains_key(name) {
                return Err(format!("{FILE}: {what} takes the shape {name}, which is not one of its shapes"));
            }
        }
        if self.hub_after < 2 {
            return Err(format!("{FILE}: hub_after {} is not 2 or more", self.hub_after));
        }
        let lines = &self.lines;
        if !(1.0..=4.0).contains(&lines.width) || !(0.0..=4.0).contains(&lines.flow) || !(0.05..=1.0).contains(&lines.glow) {
            return Err(format!("{FILE}: lines {} wide (1 to 4), flowing {} a second (0 to 4), lit over {} (0.05 to 1)", lines.width, lines.flow, lines.glow));
        }
        if !(4..=64).contains(&self.grid.spacing) || !(0.0..=1.0).contains(&self.grid.alpha) {
            return Err(format!("{FILE}: grid dots {} apart (4 to 64) at {} (0 to 1)", self.grid.spacing, self.grid.alpha));
        }
        Ok(())
    }

    /// Every quest it names a shape for is a quest: a misspelt name fails the launch rather
    /// than leaving the quest in the usual shape.
    pub fn unknown_quests<'a>(&'a self, known: impl Fn(&str) -> bool + 'a) -> impl Iterator<Item = &'a str> + 'a {
        self.quests.keys().map(String::as_str).filter(move |name| !known(name))
    }

    /// The shape `quest` stands in: its own, else a path's first quest's, a hub's (one
    /// `hub_after` quests come after), else the usual.
    pub fn shape_of<'a>(&'a self, quest: &QuestDef, first: bool, followers: usize) -> (&'a str, &'a Shape) {
        let name = self.quests.get(&quest.name).unwrap_or(if first {
            &self.start
        } else if followers >= self.hub_after {
            &self.hub
        } else {
            &self.shape
        });
        (name, &self.shapes[name])
    }

    pub fn tones(&self, look: Look) -> Tones {
        match look {
            Look::Waiting => self.waiting,
            Look::Open => self.open,
            Look::Done => self.done,
            Look::Ready => self.ready,
        }
    }
}

/// How far `p` (from the shape's middle, its radius 1) is outside the shape: below nought
/// inside. The shape is grown till its widest reaches the box's edge, as a circle's does:
/// a square fills its box rather than standing in it.
#[cfg(test)]
fn distance(shape: &Shape, p: Vec2) -> f32 {
    let reach = reach(shape);
    outline(shape, p * reach) / reach
}

/// How far across its box the shape reaches as drawn by [`outline`], at the most, either
/// way from its middle.
fn reach(shape: &Shape) -> f32 {
    (0..360)
        .map(|degree| {
            let way = Vec2::from_angle((degree as f32).to_radians());
            // Out along `way` to the edge, halving the step.
            let (mut inside, mut outside) = (0.0f32, 2.0f32);
            for _ in 0..24 {
                let middle = (inside + outside) / 2.0;
                if outline(shape, way * middle) < 0.0 { inside = middle } else { outside = middle }
            }
            (way * inside).abs().max_element()
        })
        .fold(0.0f32, f32::max)
}

/// How far `p` is outside the shape at its own size, its corners (or a circle) a radius of
/// one from its middle.
fn outline(shape: &Shape, p: Vec2) -> f32 {
    let p = Vec2::from_angle(-shape.turn.to_radians()).rotate(p);
    if shape.sides < 3 {
        return p.length() - 1.0;
    }
    let n = f32::from(shape.sides);
    if shape.teeth > 0.0 {
        // A wheel and its teeth, one a side: the wheel `teeth` short of the edge, and each
        // tooth the wedge of the whole disc a little over half its side wide, measured
        // across along the arc, so its flanks are edges as its top is.
        let (radius, angle) = (p.length(), p.y.atan2(p.x));
        let from_tooth = ((angle * n + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).abs();
        let flank = (from_tooth - TOOTH) / n * radius;
        let tooth = (radius - 1.0).max(flank);
        return tooth.min(radius - (1.0 - shape.teeth));
    }
    // A regular polygon of circumradius `r`, its corners rounded by `round`: the polygon
    // drawn that much smaller, grown back by it.
    let r = 1.0 - shape.round;
    let half = std::f32::consts::PI / n;
    let (cos, sin) = (half.cos(), half.sin());
    let sector = (p.x.atan2(p.y)).rem_euclid(2.0 * half) - half;
    let mut q = p.length() * Vec2::new(sector.cos(), sector.sin().abs()) - r * Vec2::new(cos, sin);
    q.y += (-q.y).clamp(0.0, r * sin);
    q.length() * q.x.signum() - shape.round
}

/// How far round a cog's tooth reaches either side of its middle, in a side's turn (pi):
/// a little over half the side.
const TOOTH: f32 = 1.72;

/// The texels of the rim, inside the outline.
pub const RIM: f32 = 2.0;

/// `shape` drawn `texels` across: its face, white, a little lighter at the top, to tint by
/// how the quest stands; and its rim, white on the lit upper left and grey on the shaded
/// lower right, with a black outline outside it, to tint apart.
pub fn draw(shape: &Shape, texels: u32) -> (Rgba, Rgba) {
    let size = texels.max(4);
    let mut face = Rgba { width: size, height: size, pixels: vec![0; (size * size * 4) as usize] };
    let mut rim = face.clone();
    // A texel's worth of the shape's radius; the outline sits inside the image.
    let radius = size as f32 / 2.0 - 1.0;
    let reach = reach(shape);
    for y in 0..size {
        for x in 0..size {
            let at = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - Vec2::splat(size as f32 / 2.0);
            let d = outline(shape, Vec2::new(at.x, -at.y) / radius * reach) / reach * radius;
            let i = ((y * size + x) * 4) as usize;
            if d < -RIM {
                let light = (255.0 - 40.0 * y as f32 / size as f32) as u8;
                face.pixels[i..i + 4].copy_from_slice(&[light, light, light, 255]);
            } else if d < 0.0 {
                // Lit from the upper left.
                let lit = at.x + at.y < 0.0;
                let tone = if lit { 255 } else { 176 };
                rim.pixels[i..i + 4].copy_from_slice(&[tone, tone, tone, 255]);
            } else if d < 1.0 {
                rim.pixels[i..i + 4].copy_from_slice(&[6, 7, 12, 255]);
            }
        }
    }
    (face, rim)
}

/// The whole of a shape, white, from its face and rim: what glows round the quest picked.
pub fn solid(face: &Rgba, rim: &Rgba) -> Rgba {
    let mut whole = face.clone();
    for (texel, rim) in whole.pixels.chunks_exact_mut(4).zip(rim.pixels.chunks_exact(4)) {
        if texel[3] > 0 || rim[3] > 0 {
            texel.copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    whole
}

/// The web's ground, one tile of `spacing` texels: a dot in its corner, the rest clear.
pub fn grid_tile(grid: &Grid) -> Rgba {
    let size = grid.spacing;
    let mut tile = Rgba { width: size, height: size, pixels: vec![0; (size * size * 4) as usize] };
    let [r, g, b] = [grid.dot.0, grid.dot.1, grid.dot.2];
    tile.pixels[0..4].copy_from_slice(&[r, g, b, (grid.alpha * 255.0).round() as u8]);
    tile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> QuestLook {
        let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/core").join(FILE)).unwrap();
        QuestLook::load(&[ContentFile { package: "core".into(), name: FILE.into(), source }]).expect("the shipped quest look loads")
    }

    fn filled(image: &Rgba) -> Vec<(u32, u32)> {
        (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).filter(|&(x, y)| image.pixels[((y * image.width + x) * 4 + 3) as usize] > 0).collect()
    }

    /// Every shipped shape draws at every size the book shows it: a face inside a rim
    /// inside an outline (the face never touches the image's edge), even left to right.
    #[test]
    fn every_shape_draws_a_face_in_a_rim_at_every_size() {
        let look = shipped();
        let sizes = crate::hud::quests::shape_texels(&crate::hud::quests::tests::style());
        assert!(sizes.len() >= 3, "{sizes:?}");
        for (name, shape) in &look.shapes {
            for &texels in &sizes {
                let (face, rim) = draw(shape, texels);
                let (face, rim) = (filled(&face), filled(&rim));
                assert!(face.len() > (texels * texels / 5) as usize, "{name} at {texels}: a face of {} texels", face.len());
                assert!(face.iter().all(|&(x, y)| x > 1 && y > 1 && x < texels - 2 && y < texels - 2), "{name} at {texels}: the face reaches the edge");
                assert!(face.iter().all(|at| !rim.contains(at)), "{name}: rim and face overlap");
                // Whole and even: what is drawn is its own mirror image left to right.
                let drawn: Vec<(u32, u32)> = face.iter().chain(&rim).copied().collect();
                let unmatched = drawn.iter().filter(|&&(x, y)| !drawn.contains(&(texels - 1 - x, y))).count();
                assert!(unmatched <= drawn.len() / 50, "{name} at {texels}: {unmatched} of {} texels without their mirror", drawn.len());
            }
        }
    }

    /// The shapes are told apart: a circle is not a square, a diamond stands on its point,
    /// a cog has notches round its edge.
    #[test]
    fn shapes_differ_as_named() {
        let at = |shape: Shape, p: Vec2| distance(&shape, p);
        let corner = Vec2::splat(0.68);
        assert!(at(Shape { sides: 4, turn: 45.0, ..default() }, corner) < 0.0, "a square's corner is inside it");
        assert!(at(Shape::default(), corner) < 0.0 && at(Shape::default(), Vec2::splat(0.75)) > 0.0, "a circle's edge is a radius out");
        assert!(at(Shape { sides: 4, ..default() }, Vec2::new(0.0, 0.95)) < 0.0 && at(Shape { sides: 4, ..default() }, corner) > 0.0, "a diamond stands on its points");
        let cog = Shape { sides: 10, teeth: 0.16, ..default() };
        let rim: Vec<bool> = (0..40).map(|k| at(cog, Vec2::from_angle(k as f32 / 40.0 * std::f32::consts::TAU) * 0.92) < 0.0).collect();
        assert!(rim.iter().any(|&inside| inside) && rim.iter().any(|&inside| !inside), "a cog's edge has teeth and notches: {rim:?}");
    }

    /// Each quest takes its own shape, else a path's first quest's, a hub's, the usual.
    #[test]
    fn each_quest_takes_its_shape() {
        let look = shipped();
        let quest = |name: &str| -> QuestDef { ron::from_str(&format!("(name: \"{name}\", age: \"nomad\", branch: \"main\", title: \"T\", why: \"W\", tasks: [])")).unwrap() };
        assert_eq!(look.shape_of(&quest("base:homestead_hearth"), true, 9).0, "cog");
        assert_eq!(look.shape_of(&quest("base:a"), true, 9).0, look.start);
        assert_eq!(look.shape_of(&quest("base:a"), false, look.hub_after).0, look.hub);
        assert_eq!(look.shape_of(&quest("base:a"), false, 1).0, look.shape);
    }

    /// A later package changes one shape and adds one, and gives a quest its own; one
    /// that names a shape nothing gives, or draws a shape that cannot be, fails.
    #[test]
    fn a_later_package_restyles_and_bad_looks_fail() {
        let core = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/core").join(FILE)).unwrap();
        let file = |package: &str, source: &str| ContentFile { package: package.into(), name: FILE.into(), source: source.into() };
        let look = QuestLook::load(&[file("core", &core), file("star", r#"(shapes: {"star": (sides: 5), "circle": (sides: 12)}, quests: {"base:a": "star"}, lines: (width: 3, done: (1, 2, 3), ahead: (4, 5, 6), flow: 1, glow: 0.2, light: (7, 8, 9)))"#)]).unwrap();
        assert_eq!(look.shapes["star"].sides, 5);
        assert_eq!(look.shapes["circle"].sides, 12);
        assert_eq!(look.quests["base:a"], "star");
        assert_eq!(look.lines.width, 3.0);
        assert_eq!(look.shape, "rounded", "what it leaves is the earlier package's");
        // Each fails naming what is wrong, in the file's name.
        for (bad, says) in [
            (r#"(quests: {"base:a": "blob"})"#, "base:a takes the shape blob, which is not one of its shapes"),
            (r#"(shapes: {"line": (sides: 2)})"#, "the shape line has 2 sides"),
            (r#"(shapes: {"spiky": (sides: 6, teeth: 0.7)})"#, "has teeth 0.7 (0 to 0.4)"),
            (r#"(shapes: {"bare": (teeth: 0.2)})"#, "the shape bare has teeth but no sides"),
            (r#"(shape: "blob")"#, "shape takes the shape blob"),
            (r#"(hub_after: 1)"#, "hub_after 1 is not 2 or more"),
            (r#"(grid: (spacing: 2, dot: (1, 1, 1), alpha: 0.5))"#, "grid dots 2 apart (4 to 64)"),
            (r#"(lines: (width: 9, done: (1, 2, 3), ahead: (4, 5, 6), flow: 1, glow: 0.2, light: (7, 8, 9)))"#, "lines 9 wide (1 to 4)"),
            (r#"(lines: (width: 2, done: (1, 2, 3), ahead: (4, 5, 6), flow: 1, glow: 0.2))"#, "bad/quest_book.ron"),
            (r#"(sparkle: true)"#, "bad/quest_book.ron"),
        ] {
            let error = QuestLook::load(&[file("core", &core), file("bad", bad)]).err().unwrap_or_else(|| panic!("{bad} loaded"));
            assert!(error.contains(FILE) && error.contains(says), "{bad}: {error}");
        }
        assert!(look.unknown_quests(|name| name == "base:homestead_hearth" || name == "base:crucible_kiln").eq(["base:a"]));
    }
}
