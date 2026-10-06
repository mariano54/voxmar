//! The recipe language, as written in a pack's `recipes/*.ron` books.
//!
//! A texture is a stack of layers painted onto a canvas. Most layers paint with a named
//! palette (a ramp of shades, darkest first): a [`Paint`] layer adds up scalar [`Term`]s
//! per texel and picks the shade, so a stone is noise, a cobblestone is cells with dark
//! mortar and a plank is boards with grain. [`Grid`] layers are text sprites: rows of
//! characters, each meaning a colour, a palette shade or a change to what is beneath.
//! Texels remember which palette shade they hold, so later layers can lighten, darken or
//! cut through them.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One recipe file. Every texture name is global within a pack.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Book {
    /// Palettes recipes can refer to by name.
    pub palettes: BTreeMap<String, Palette>,
    /// Textures by path, such as `block/stone` or `item/apple`.
    pub textures: BTreeMap<String, Recipe>,
    /// Sets of textures made together: texture path pattern (with `{name}`) to recipe.
    pub templates: BTreeMap<String, BTreeMap<String, Recipe>>,
    /// Template instances, such as one wood made with the `wood` template.
    pub families: Vec<Family>,
    /// Characters every [`Grid`] in this book understands, unless its own legend says
    /// otherwise: a book of plant sprites can agree that `a`–`e` are leaf shades once.
    pub legend: BTreeMap<char, String>,
}

/// A palette: a list of `#rrggbb` shades (darkest first), a ramp built from one colour, or
/// the name of a palette defined in any book of the pack.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Palette {
    Named(String),
    Shades(Vec<String>),
    Ramp(Ramp),
}

/// Hue-shifted shades around one key colour.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ramp {
    /// The middle shade.
    pub ramp: String,
    pub shades: u32,
    /// OKLab lightness from the darkest to the lightest shade.
    pub light: f32,
    /// Degrees the darkest shade turns towards blue and the lightest towards yellow.
    pub hue: f32,
    /// Relative chroma change from the middle to the lightest shade (negative pales the
    /// highlights and deepens the shadows).
    pub chroma: f32,
}

impl Default for Ramp {
    fn default() -> Self {
        Self { ramp: String::new(), shades: 5, light: 0.32, hue: 24.0, chroma: -0.3 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    /// A texture to start from: its size, seed, palettes and layers are inherited, then
    /// this recipe's palettes replace those of the same name and its layers are added on
    /// top.
    pub base: Option<String>,
    /// Width and height; 16×16 unless inherited.
    pub size: Option<(u32, u32)>,
    /// Seeds every random choice. Unless inherited or given, derived from the name.
    pub seed: Option<u32>,
    pub palettes: BTreeMap<String, Palette>,
    pub layers: Vec<Layer>,
}

/// A use of a template: every texture in it, with `{name}` replaced, drawn with these
/// palettes. Textures defined explicitly anywhere in the pack take precedence.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Family {
    pub template: String,
    pub name: String,
    pub palettes: BTreeMap<String, Palette>,
    /// Seed for the family's textures; derived from its name when absent, so two woods
    /// made from one template still differ.
    pub seed: Option<u32>,
    /// Template entries to leave out, by pattern (`block/{name}_sapling`).
    pub without: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Layer {
    Paint(Paint),
    Grid(Grid),
    Clusters(Clusters),
    Shift(Shift),
    Cut(Cut),
    Stamp(Stamp),
    /// Four corner colours (top-left, top-right, bottom-left, bottom-right), blended
    /// smoothly: colour maps and gradients. Written `Corners("#…", "#…", "#…", "#…")`.
    Corners([String; 4]),
}

/// Paints with a palette: each texel's shade comes from `base` plus the sum of `shade`,
/// where 0 is the darkest shade and 1 just past the lightest.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Paint {
    pub palette: String,
    pub base: f32,
    pub shade: Vec<Term>,
    /// Paint only where the sum of these terms is above `above` (everywhere when empty).
    pub mask: Vec<Term>,
    pub above: f32,
    /// Replace lone texels whose four neighbours all share another shade, the way a pixel
    /// artist would.
    pub clean: bool,
    /// Shift the texels just below and right of the painted area by this many shades
    /// (negative darkens), so it stands out from what it was painted on.
    pub shadow: i32,
    /// Shift the texels just above and left of the painted area: a crack painted dark
    /// with `light: 1` gets the bright lip a pixel artist would give it.
    pub light: i32,
    /// Make texels whose shade comes out below this transparent instead: the gaps
    /// between leaf clumps.
    pub cut: Option<f32>,
}

impl Default for Paint {
    fn default() -> Self {
        Self { palette: String::new(), base: 0.5, shade: Vec::new(), mask: Vec::new(), above: 0.5, clean: false, shadow: 0, light: 0, cut: None }
    }
}

/// A text sprite. Each character of `rows` looks itself up in `legend`:
/// `#rrggbb[aa]` paints a colour, `name:3` a palette shade (0 darkest), `+1` or `-2`
/// lightens or darkens what is beneath by shades, and `clear` makes it transparent.
/// Characters not in the legend (conventionally `.`) leave the texel as it is.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grid {
    pub legend: BTreeMap<char, String>,
    pub rows: Vec<String>,
    /// Top-left corners to draw the grid at; once at (0, 0) when empty.
    pub at: Vec<(i32, i32)>,
    /// Draw the grid mirrored left to right.
    pub mirror: bool,
    /// Wrap what runs past an edge round to the opposite one, so the texture still tiles.
    pub wrap: bool,
}

/// Another texture drawn over this one at each offset (its clear texels leave what is
/// beneath): a clump of three blooms is one bloom stamped three times.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stamp {
    pub texture: String,
    /// Where its top-left corner goes; (0, 0) when empty.
    pub at: Vec<(i32, i32)>,
    /// Mirror it left to right.
    pub mirror: bool,
    /// Shift the texels just below and right of what it drew by this many shades, as
    /// `Paint` does: a grass block's side is its overlay's fringe stamped on dirt, with
    /// the dirt darkened under it.
    pub shadow: i32,
    /// Read `at` in sixteenths of the stamped texture's width rather than in texels, and
    /// take a recipe's size from the texture it stamps first when it gives none: a
    /// template that stamps a flower twice works for 16×16 and 32×32 flowers alike.
    pub relative: bool,
}

/// Small blobs of a palette scattered over the texture (ore, pebbles, lichen, petals).
/// Each blob is lit from the top left: its top-left edge takes lighter shades and its
/// bottom-right edge darker ones.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clusters {
    pub palette: String,
    pub count: u32,
    /// Texels per blob, smallest and largest.
    pub size: (u32, u32),
    /// How readily a blob grows sideways and downwards (1, 1 is round).
    pub spread: (f32, f32),
    /// Minimum distance between blob centres, in texels.
    pub spacing: f32,
    /// The shade of a blob's interior; the middle of the palette when absent.
    pub shade: Option<u32>,
    /// Shades darker (negative) for the texels below and right of each blob.
    pub shadow: i32,
    /// Keep blobs this far from the texture's edges (0 lets them wrap around).
    pub margin: u32,
    /// How strongly a blob keeps to a round outline as it grows (0 packs texels anywhere
    /// they touch most, which makes big blobs square). Blobs of more than 16 texels get
    /// 2.5 when absent, so 16×16 recipes drawn finer keep round pebbles.
    pub round: Option<f32>,
    pub seed: u32,
}

impl Default for Clusters {
    fn default() -> Self {
        Self {
            palette: String::new(),
            count: 4,
            size: (3, 6),
            spread: (1.0, 1.0),
            spacing: 4.0,
            shade: None,
            shadow: 0,
            margin: 0,
            round: None,
            seed: 0,
        }
    }
}

/// Lightens or darkens painted texels by `by` shades (rounded), where the mask allows.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shift {
    pub by: Vec<Term>,
    pub mask: Vec<Term>,
    pub above: f32,
}

impl Default for Shift {
    fn default() -> Self {
        Self { by: Vec::new(), mask: Vec::new(), above: 0.5 }
    }
}

/// Makes texels transparent where the sum of `mask` is above `above` (or, with `below`,
/// below it). With `darker_than`, only painted texels of a shade below it are cut: the dark
/// gaps between a leaf texture's clusters, so its holes open between leaves, not through
/// them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cut {
    pub mask: Vec<Term>,
    pub above: Option<f32>,
    pub below: Option<f32>,
    pub darker_than: Option<u8>,
}

/// A scalar value per texel. Every pattern tiles seamlessly.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Term {
    /// The same everywhere.
    Flat(f32),
    /// Smooth value noise in -amount/2..amount/2.
    Noise(Noise),
    /// Thin lines where noise crosses zero, `amount` on the line fading to 0 at `width`.
    Ridge(Ridge),
    /// Each texel independently gets `amount` with probability `chance`.
    Speckle(Speckle),
    /// Each texel independently gets a value in -amount/2..amount/2: the per-texel grain
    /// of soil, grass and leaves (`Grain(0.6)`, or `Grain(0.6, 3)` with a seed).
    Grain(f32, #[serde(default)] u32),
    /// Irregular cells (stones, pebbles, leaf clumps).
    Cells(Cells),
    /// Brick courses.
    Bricks(Bricks),
    /// Boards laid horizontally, with end joints.
    Boards(Boards),
    /// Growth rings around a point.
    Rings(Rings),
    /// Parallel bands (bedding, layers), optionally wavy.
    Stripes(Stripes),
    /// `amount` within `width` texels of the edge (each side can be weighted).
    Border(Border),
    /// Top and left edges lighter, bottom and right darker: a raised tile.
    Bevel(Bevel),
    /// 1 in a ragged band hanging from the top (or rising from the bottom), 0 elsewhere.
    Fringe(Fringe),
    /// A straight ramp from `from` to `to`, down the texture (or across it).
    Gradient(Gradient),
    /// 1 inside a circle, fading to 0 over `soft` texels; ragged by `rough`.
    Circle(Circle),
    /// An ordered 4×4 dither pattern in -amount/2..amount/2, for pixel-art blends.
    Dither(f32),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Noise {
    /// Lattice cells across and down the texture; more cells, finer noise.
    pub cells: (u32, u32),
    pub octaves: u32,
    /// Amplitude of each octave relative to the previous one.
    pub gain: f32,
    pub amount: f32,
    pub seed: u32,
}

impl Default for Noise {
    fn default() -> Self {
        Self { cells: (4, 4), octaves: 1, gain: 0.5, amount: 1.0, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ridge {
    pub cells: (u32, u32),
    pub octaves: u32,
    pub width: f32,
    pub amount: f32,
    pub seed: u32,
}

impl Default for Ridge {
    fn default() -> Self {
        Self { cells: (3, 3), octaves: 1, width: 0.08, amount: 1.0, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Speckle {
    pub chance: f32,
    pub amount: f32,
    pub seed: u32,
}

impl Default for Speckle {
    fn default() -> Self {
        Self { chance: 0.1, amount: 1.0, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cells {
    /// Cells across (and down) the texture; they sit on a jittered grid.
    pub count: (u32, u32),
    /// How far a cell's centre may stray from its grid spot, 0..1.
    pub jitter: f32,
    /// Each cell's own shade varies by up to ±value/2.
    pub value: f32,
    /// Added on cell boundaries.
    pub edge: f32,
    /// Boundary width in texels.
    pub width: f32,
    /// Lighting from the top left across each cell.
    pub bevel: f32,
    /// The lit texel row and column along each cell's top and left boundary; `bevel` when
    /// absent. 0 shades each cell smoothly with no lit line, for large stylised shapes.
    pub lip: Option<f32>,
    /// Darkening towards each cell's rim, which makes cells read as rounded stones.
    pub round: f32,
    /// How far boundaries wander, in texels, so cells are not straight-edged polygons.
    pub warp: f32,
    /// Stretches every cell across and down: (1, 3) makes tall strips (bark plates),
    /// (3, 1) wide ones.
    pub stretch: (f32, f32),
    pub seed: u32,
}

impl Default for Cells {
    fn default() -> Self {
        Self { count: (3, 3), jitter: 0.8, value: 0.3, edge: -0.4, width: 1.0, bevel: 0.3, lip: None, round: 0.0, warp: 0.0, stretch: (1.0, 1.0), seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bricks {
    /// Brick size including one texel of mortar.
    pub size: (u32, u32),
    /// Horizontal shift of every other course; half a brick when absent.
    pub offset: Option<u32>,
    pub mortar: f32,
    pub value: f32,
    pub bevel: f32,
    pub seed: u32,
}

impl Default for Bricks {
    fn default() -> Self {
        Self { size: (8, 4), offset: None, mortar: -0.4, value: 0.2, bevel: 0.15, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Boards {
    /// Board height including the seam texel.
    pub height: u32,
    /// The dark row under each board.
    pub seam: f32,
    /// The dark texel where two boards meet end to end, one or two per board.
    pub joint: f32,
    /// Each board's own shade varies by up to ±value/2.
    pub value: f32,
    /// Lighter top row of each board.
    pub bevel: f32,
    /// A nail hole near each joint, this much darker.
    pub nails: f32,
    pub seed: u32,
}

impl Default for Boards {
    fn default() -> Self {
        Self { height: 4, seam: -0.45, joint: -0.35, value: 0.15, bevel: 0.1, nails: 0.0, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rings {
    /// The centre, in texels; the middle of the texture when absent.
    pub centre: Option<(f32, f32)>,
    /// Texels from one ring to the next.
    pub spacing: f32,
    /// Fraction of each period that is the dark ring.
    pub width: f32,
    pub amount: f32,
    /// How much the rings wander.
    pub wobble: f32,
    /// Squares the rings off towards the texture's shape (0 round, 1 square).
    pub square: f32,
    pub seed: u32,
}

impl Default for Rings {
    fn default() -> Self {
        Self { centre: None, spacing: 2.5, width: 0.4, amount: -0.3, wobble: 0.6, square: 0.3, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stripes {
    /// Texels per band; the texture's height should be a multiple.
    pub period: f32,
    /// Fraction of each period that gets `amount`.
    pub width: f32,
    pub amount: f32,
    /// How far the bands wave, in texels.
    pub warp: f32,
    /// Bands run up and down instead of across.
    pub vertical: bool,
    /// Where the first band starts, in texels.
    pub phase: f32,
    /// Texels the bands climb per texel across, for diagonal streaks. Keep `slope` ×
    /// the texture's width a multiple of `period` so the bands still tile.
    pub slope: f32,
    pub seed: u32,
}

impl Default for Stripes {
    fn default() -> Self {
        Self { period: 4.0, width: 0.25, amount: -0.3, warp: 0.0, vertical: false, phase: 0.0, slope: 0.0, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Border {
    pub width: u32,
    pub amount: f32,
    /// Weights of the top, right, bottom and left edges.
    pub sides: (f32, f32, f32, f32),
}

impl Default for Border {
    fn default() -> Self {
        Self { width: 1, amount: -0.3, sides: (1.0, 1.0, 1.0, 1.0) }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bevel {
    pub width: u32,
    pub amount: f32,
}

impl Default for Bevel {
    fn default() -> Self {
        Self { width: 1, amount: 0.2 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fringe {
    /// Shortest and longest reach, in texels.
    pub depth: (u32, u32),
    /// Grow up from the bottom edge instead.
    pub bottom: bool,
    /// How much neighbouring columns agree (0 ragged, 1 smooth).
    pub smooth: f32,
    pub seed: u32,
}

impl Default for Fringe {
    fn default() -> Self {
        Self { depth: (2, 5), bottom: false, smooth: 0.3, seed: 0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gradient {
    pub from: f32,
    pub to: f32,
    /// Left to right instead of top to bottom.
    pub across: bool,
}

impl Default for Gradient {
    fn default() -> Self {
        Self { from: 0.0, to: 1.0, across: false }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Circle {
    /// The middle of the texture when absent.
    pub centre: Option<(f32, f32)>,
    pub radius: f32,
    pub soft: f32,
    /// Texels the edge wanders in and out.
    pub rough: f32,
    pub seed: u32,
}

impl Default for Circle {
    fn default() -> Self {
        Self { centre: None, radius: 6.0, soft: 0.0, rough: 0.0, seed: 0 }
    }
}
