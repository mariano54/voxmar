//! Pixel fonts written as text, built into TrueType fonts that any text engine can lay out.
//!
//! A font file draws each glyph as rows of `#` (ink) and `.` (paper), the same way a
//! texture recipe's grid draws a sprite. The first row of a glyph is the top of a capital
//! letter; a glyph lists as many rows as it needs, down to two below the baseline for
//! descenders. Accented letters are composed from a base letter and a mark, so the file
//! draws `a` and `´` once and gets `á`.
//!
//! ```text
//! // A comment.
//! = A          a glyph: `=`, a space, the character
//! .###.
//! #...#
//! …
//! = space 3    a blank glyph three pixels wide
//! mark acute   a mark, drawn over the letters that compose with it
//! ..#
//! .#.
//! mark cedilla below   a mark drawn under the baseline instead
//! compose á a acute
//! monospace 5  every glyph in a cell five pixels wide, narrower ones centred
//! include x.txt  read another file here (with `PixelFont::parse_with`)
//! ```
//!
//! [`PixelFont::to_ttf`] traces each glyph's pixels into outlines (one contour around each
//! connected run of ink, holes and all) at [`UNITS_PER_PIXEL`] font units per pixel, with
//! an em of [`EM_PIXELS`] pixels. A font size of `EM_PIXELS × n` therefore draws every
//! pixel as an n×n square, crisp with antialiasing off.

use std::collections::BTreeMap;

use crate::Error;

/// Rows from the baseline to the top of a capital letter.
pub const CAP_HEIGHT: i32 = 7;
/// Rows above the baseline the font reserves: capitals and the marks over them.
pub const ASCENT: i32 = 9;
/// Rows below the baseline the font reserves, for descenders.
pub const DESCENT: i32 = 2;
/// Pixels per em: a font size of 8 draws one screen pixel per font pixel, 16 two.
pub const EM_PIXELS: i32 = 8;
pub const UNITS_PER_PIXEL: i32 = 128;
/// Pixels of space after every glyph.
const SPACING: i32 = 1;
/// Rows between a lower-case letter's top and its mark, and a capital's.
const LOWER_MARK_TOP: i32 = 0;
const UPPER_MARK_TOP: i32 = -2;
/// Rows from the top of a capital to the top of a lower-case letter.
const X_HEIGHT_TOP: i32 = 2;

/// A glyph's ink: `rows[r][x]`, with row 0 at the top of a capital letter and `top` rows
/// above that (negative for marks over capitals).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: i32,
    pub top: i32,
    pub rows: Vec<Vec<bool>>,
}

impl Bitmap {
    fn blank(width: i32) -> Self {
        Self { width, top: 0, rows: Vec::new() }
    }

    fn ink(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.rows.iter().enumerate().flat_map(move |(r, row)| {
            row.iter().enumerate().filter(|(_, ink)| **ink).map(move |(x, _)| (x as i32, self.top + r as i32))
        })
    }

    /// The first row with ink, if any.
    fn first_row(&self) -> Option<i32> {
        self.ink().map(|(_, row)| row).min()
    }

    /// Draws `other` over this bitmap with its left edge at `x` and its row 0 at `row`.
    fn stamp(&mut self, other: &Bitmap, x: i32, row: i32) {
        for (ox, orow) in other.ink() {
            let (px, r) = (ox + x, orow + row);
            if r < self.top {
                let missing = (self.top - r) as usize;
                self.rows.splice(0..0, std::iter::repeat_n(vec![false; self.width as usize], missing));
                self.top = r;
            }
            let index = (r - self.top) as usize;
            if self.rows.len() <= index {
                self.rows.resize(index + 1, vec![false; self.width as usize]);
            }
            if (0..self.width).contains(&px) {
                self.rows[index][px as usize] = true;
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PixelFont {
    /// Glyphs by character.
    pub glyphs: BTreeMap<char, Bitmap>,
    /// The cell width of a monospace font: every glyph advances as far, and narrower ones
    /// sit in its middle.
    pub monospace: Option<i32>,
    /// Font units per pixel of glyphs drawn on a finer grid than the font's own (imported
    /// kanji shown smaller than our letters, see `import`); every other glyph has
    /// [`UNITS_PER_PIXEL`].
    pub units: BTreeMap<char, i32>,
}

impl PixelFont {
    /// Reads a font file (see the module docs).
    pub fn parse(source: &str) -> Result<Self, Error> {
        Self::parse_with(source, |_| None)
    }

    /// Reads a font file whose `include` lines name other files, which `read` returns.
    pub fn parse_with(source: &str, read: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        let mut text = String::new();
        expand(source, &read, &mut text, 0)?;
        let mut font = Self::parse_lines(&text)?;
        font.monospace = text
            .lines()
            .find_map(|line| line.strip_prefix("monospace "))
            .map(|cell| cell.trim().parse().map_err(|_| Error("font: `monospace` takes a width in pixels".into())))
            .transpose()?;
        if let Some(cell) = font.monospace {
            for glyph in font.glyphs.values_mut() {
                if glyph.width < cell {
                    let mut centred = Bitmap::blank(cell);
                    centred.stamp(glyph, (cell - glyph.width) / 2, 0);
                    *glyph = centred;
                }
            }
        }
        Ok(font)
    }

    fn parse_lines(source: &str) -> Result<Self, Error> {
        let mut font = PixelFont::default();
        let mut marks: BTreeMap<String, Mark> = BTreeMap::new();
        // The glyph or mark whose rows are being read.
        enum Open {
            None,
            Glyph(char),
            Mark(String, bool),
        }
        let mut open = Open::None;
        let mut rows: Vec<Vec<bool>> = Vec::new();
        let close = |open: &Open, rows: &mut Vec<Vec<bool>>, font: &mut PixelFont, marks: &mut BTreeMap<String, Mark>| -> Result<(), Error> {
            let taken = std::mem::take(rows);
            let width = taken.first().map_or(0, |row| row.len() as i32);
            if taken.iter().any(|row| row.len() as i32 != width) {
                let name = match open {
                    Open::Glyph(c) => format!("`{c}`"),
                    Open::Mark(name, _) => format!("mark {name}"),
                    Open::None => String::new(),
                };
                return Err(Error(format!("font: rows of {name} differ in width")));
            }
            match open {
                Open::None => {}
                Open::Glyph(c) => {
                    if !taken.is_empty() {
                        font.glyphs.insert(*c, Bitmap { width, top: 0, rows: taken });
                    }
                }
                Open::Mark(name, below) => {
                    marks.insert(name.clone(), Mark { bitmap: Bitmap { width, top: 0, rows: taken }, below: *below });
                }
            }
            Ok(())
        };
        for (number, line) in source.lines().enumerate() {
            let line = line.trim_end();
            let bad = |what: &str| Error(format!("font line {}: {what}", number + 1));
            if line.starts_with("//") || line.trim().is_empty() || line.starts_with("monospace ") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("= ") {
                close(&open, &mut rows, &mut font, &mut marks)?;
                let mut words = rest.split_whitespace();
                let name = words.next().ok_or_else(|| bad("a glyph needs a character"))?;
                let c = match name {
                    "space" => ' ',
                    _ if name.chars().count() == 1 => name.chars().next().unwrap(),
                    _ => return Err(bad("a glyph is one character, or `space`")),
                };
                if let Some(width) = words.next() {
                    let width = width.parse().map_err(|_| bad("a blank glyph's width is a number"))?;
                    font.glyphs.insert(c, Bitmap::blank(width));
                    open = Open::None;
                } else {
                    open = Open::Glyph(c);
                }
            } else if let Some(rest) = line.strip_prefix("mark ") {
                close(&open, &mut rows, &mut font, &mut marks)?;
                let words: Vec<&str> = rest.split_whitespace().collect();
                open = match words[..] {
                    [name] => Open::Mark(name.to_owned(), false),
                    [name, "below"] => Open::Mark(name.to_owned(), true),
                    _ => return Err(bad("a mark is `mark NAME` or `mark NAME below`")),
                };
            } else if let Some(rest) = line.strip_prefix("compose ") {
                close(&open, &mut rows, &mut font, &mut marks)?;
                open = Open::None;
                let words: Vec<&str> = rest.split_whitespace().collect();
                let [made, base, mark] = words[..] else { return Err(bad("compose needs a character, a base and a mark")) };
                let (Some(made), Some(base)) = (made.chars().next(), base.chars().next()) else { return Err(bad("compose needs characters")) };
                let base = font.glyphs.get(&base).ok_or_else(|| bad("compose's base is not drawn yet"))?.clone();
                let mark = marks.get(mark).ok_or_else(|| bad("no such mark"))?;
                font.glyphs.insert(made, compose(&base, mark));
            } else if line.chars().all(|c| c == '#' || c == '.') {
                if matches!(open, Open::None) {
                    return Err(bad("pixels outside a glyph"));
                }
                rows.push(line.chars().map(|c| c == '#').collect());
            } else {
                return Err(bad("expected `= c`, `mark`, `compose`, a comment or a row of `#` and `.`"));
            }
        }
        close(&open, &mut rows, &mut font, &mut marks)?;
        Ok(font)
    }

    /// The font as a TrueType file, named `family`.
    pub fn to_ttf(&self, family: &str) -> Vec<u8> {
        ttf::build(self, family)
    }
}

/// `source` with its `include` lines replaced by the files they name.
fn expand(source: &str, read: &impl Fn(&str) -> Option<String>, out: &mut String, depth: u32) -> Result<(), Error> {
    if depth > 8 {
        return Err(Error("font: includes nest too deep".into()));
    }
    for line in source.lines() {
        if let Some(name) = line.strip_prefix("include ") {
            let included = read(name.trim()).ok_or_else(|| Error(format!("font: cannot include {}", name.trim())))?;
            expand(&included, read, out, depth + 1)?;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(())
}

/// An accent or other mark that letters are composed with.
struct Mark {
    bitmap: Bitmap,
    /// Drawn under the baseline (a cedilla) rather than over the letter.
    below: bool,
}

/// A base letter with a mark centred on it: over the x-height for lower case (and for
/// letters such as `ı` that start there), over the capital height otherwise, or just under
/// the baseline.
fn compose(base: &Bitmap, mark: &Mark) -> Bitmap {
    let (below, mark) = (mark.below, &mark.bitmap);
    let width = base.width.max(mark.width);
    let mut made = Bitmap::blank(width);
    made.stamp(base, (width - base.width) / 2, 0);
    let row = if below {
        CAP_HEIGHT
    } else if base.first_row().unwrap_or(0) >= X_HEIGHT_TOP {
        LOWER_MARK_TOP
    } else {
        UPPER_MARK_TOP
    };
    made.stamp(mark, (width - mark.width) / 2, row);
    made
}

/// Traces the ink of a bitmap into closed outlines in font pixels, y up from the
/// baseline: clockwise around ink, counter-clockwise around holes, as TrueType wants.
fn outlines(bitmap: &Bitmap) -> Vec<Vec<(i32, i32)>> {
    use std::collections::HashMap;
    let ink: std::collections::BTreeSet<(i32, i32)> =
        bitmap.ink().map(|(x, row)| (x, CAP_HEIGHT - 1 - row)).collect();
    let filled = |x: i32, y: i32| ink.contains(&(x, y));
    // Unit edges with ink on their right, keyed by where they start.
    let mut edges: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
    let mut add = |from: (i32, i32), to: (i32, i32)| edges.entry(from).or_default().push(to);
    for &(x, y) in &ink {
        if !filled(x - 1, y) {
            add((x, y), (x, y + 1));
        }
        if !filled(x, y + 1) {
            add((x, y + 1), (x + 1, y + 1));
        }
        if !filled(x + 1, y) {
            add((x + 1, y + 1), (x + 1, y));
        }
        if !filled(x, y - 1) {
            add((x + 1, y), (x, y));
        }
    }
    let mut starts: Vec<(i32, i32)> = edges.keys().copied().collect();
    starts.sort();
    let mut contours = Vec::new();
    for start in starts {
        while let Some(first) = edges.get_mut(&start).and_then(Vec::pop) {
            let mut points = vec![start];
            let (mut from, mut at) = (start, first);
            while at != start {
                points.push(at);
                let options = edges.get_mut(&at).expect("outlines close");
                // Where two runs of ink touch only at a corner, turn right, keeping them
                // apart.
                let heading = (at.0 - from.0, at.1 - from.1);
                let right = (heading.1, -heading.0);
                let pick = options.iter().position(|&to| (to.0 - at.0, to.1 - at.1) == right).unwrap_or(0);
                let next = options.swap_remove(pick);
                (from, at) = (at, next);
            }
            contours.push(simplify(points));
        }
    }
    contours
}

/// Drops the points in the middle of straight runs.
fn simplify(points: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    let n = points.len();
    (0..n)
        .filter(|&i| {
            let (a, b, c) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
            (b.0 - a.0) * (c.1 - b.1) != (b.1 - a.1) * (c.0 - b.0)
        })
        .map(|i| points[i])
        .collect()
}

/// The TrueType container: just the tables every text engine requires, with simple glyph
/// outlines made only of on-curve points and no hinting.
mod import;
pub use import::{characters, import};

mod ttf {
    use super::*;

    struct Glyph {
        advance: u16,
        contours: Vec<Vec<(i32, i32)>>,
    }

    impl Glyph {
        fn bounds(&self) -> (i16, i16, i16, i16) {
            let points = self.contours.iter().flatten();
            let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
            for &(x, y) in points {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            if x0 > x1 { (0, 0, 0, 0) } else { (x0 as i16, y0 as i16, x1 as i16, y1 as i16) }
        }
    }

    #[derive(Default)]
    struct Out(Vec<u8>);

    impl Out {
        fn u8(&mut self, v: u8) -> &mut Self {
            self.0.push(v);
            self
        }
        fn u16(&mut self, v: u16) -> &mut Self {
            self.0.extend(v.to_be_bytes());
            self
        }
        fn i16(&mut self, v: i16) -> &mut Self {
            self.0.extend(v.to_be_bytes());
            self
        }
        fn u32(&mut self, v: u32) -> &mut Self {
            self.0.extend(v.to_be_bytes());
            self
        }
        fn pad(&mut self) {
            while !self.0.len().is_multiple_of(4) {
                self.0.push(0);
            }
        }
    }

    fn checksum(data: &[u8]) -> u32 {
        data.chunks(4).fold(0u32, |sum, word| {
            let mut bytes = [0u8; 4];
            bytes[..word.len()].copy_from_slice(word);
            sum.wrapping_add(u32::from_be_bytes(bytes))
        })
    }

    /// The largest power of two at most `n`, and its log: the binary search hints that
    /// table directories and cmap segments carry.
    fn search(n: u16, unit: u16) -> (u16, u16, u16) {
        let log = 15 - n.max(1).leading_zeros() as u16;
        let range = (1u16 << log) * unit;
        (range, log, n * unit - range)
    }

    pub fn build(font: &PixelFont, family: &str) -> Vec<u8> {
        let unit = UNITS_PER_PIXEL;
        // Glyph 0 is the box shown for characters the font lacks.
        let notdef = Bitmap {
            width: 4,
            top: 0,
            rows: ["####", "#..#", "#..#", "#..#", "#..#", "#..#", "####"]
                .iter()
                .map(|row| row.chars().map(|c| c == '#').collect())
                .collect(),
        };
        let chars: Vec<char> = font.glyphs.keys().copied().collect();
        let glyphs: Vec<Glyph> = std::iter::once((&notdef, unit))
            .chain(chars.iter().map(|c| (&font.glyphs[c], font.units.get(c).copied().unwrap_or(unit))))
            .map(|(bitmap, unit)| Glyph {
                // A finer grid's glyphs keep to their own width: the monospace cell is ours.
                advance: ((if unit == UNITS_PER_PIXEL { bitmap.width.max(font.monospace.unwrap_or(0)) } else { bitmap.width } + SPACING) * unit) as u16,
                contours: outlines(bitmap).into_iter().map(|c| c.into_iter().map(|(x, y)| (x * unit, y * unit)).collect()).collect(),
            })
            .collect();
        let count = glyphs.len() as u16;
        let ascent = (ASCENT * unit) as i16;
        let descent = (-DESCENT * unit) as i16;

        let mut glyf = Out::default();
        let mut loca = Out::default();
        for glyph in &glyphs {
            loca.u32(glyf.0.len() as u32);
            if glyph.contours.is_empty() {
                continue;
            }
            let (x0, y0, x1, y1) = glyph.bounds();
            glyf.i16(glyph.contours.len() as i16).i16(x0).i16(y0).i16(x1).i16(y1);
            let mut end = 0u16;
            for contour in &glyph.contours {
                end += contour.len() as u16;
                glyf.u16(end - 1);
            }
            glyf.u16(0); // no instructions
            let points: Vec<(i32, i32)> = glyph.contours.iter().flatten().copied().collect();
            for _ in &points {
                glyf.u8(0x01); // on curve, long x and y deltas
            }
            let mut last = (0, 0);
            for &(x, _) in &points {
                glyf.i16((x - last.0) as i16);
                last.0 = x;
            }
            for &(_, y) in &points {
                glyf.i16((y - last.1) as i16);
                last.1 = y;
            }
            glyf.pad();
        }
        loca.u32(glyf.0.len() as u32);

        let bounds: Vec<_> = glyphs.iter().filter(|g| !g.contours.is_empty()).map(Glyph::bounds).collect();
        let x_min = bounds.iter().map(|b| b.0).min().unwrap_or(0);
        let y_min = bounds.iter().map(|b| b.1).min().unwrap_or(0);
        let x_max = bounds.iter().map(|b| b.2).max().unwrap_or(0);
        let y_max = bounds.iter().map(|b| b.3).max().unwrap_or(0);
        let advance_max = glyphs.iter().map(|g| g.advance).max().unwrap_or(0);
        let max_points = glyphs.iter().map(|g| g.contours.iter().map(Vec::len).sum::<usize>()).max().unwrap_or(0) as u16;
        let max_contours = glyphs.iter().map(|g| g.contours.len()).max().unwrap_or(0) as u16;

        let mut head = Out::default();
        head.u32(0x0001_0000).u32(0x0001_0000).u32(0).u32(0x5F0F_3CF5);
        // Baseline at y = 0, left side bearing at x = 0, integer scaling of sizes.
        head.u16(0b1011).u16((EM_PIXELS * unit) as u16).u32(0).u32(0).u32(0).u32(0);
        head.i16(x_min).i16(y_min).i16(x_max).i16(y_max).u16(0).u16(EM_PIXELS as u16).i16(2).i16(1).i16(0);

        let mut hhea = Out::default();
        hhea.u32(0x0001_0000).i16(ascent).i16(descent).i16(0).u16(advance_max);
        hhea.i16(x_min.min(0)).i16(0).i16(x_max).i16(1).i16(0).i16(0);
        hhea.i16(0).i16(0).i16(0).i16(0).i16(0).u16(count);

        let mut maxp = Out::default();
        maxp.u32(0x0001_0000).u16(count).u16(max_points).u16(max_contours).u16(0).u16(0).u16(2);
        for _ in 0..8 {
            maxp.u16(0);
        }

        let mut hmtx = Out::default();
        for glyph in &glyphs {
            hmtx.u16(glyph.advance).i16(glyph.bounds().0);
        }

        // Format 12: runs of consecutive characters with consecutive glyphs, so fonts
        // with thousands of kanji fit as easily as a few dozen letters.
        let mut runs: Vec<(u32, u32, u32)> = Vec::new();
        for (i, &c) in chars.iter().enumerate() {
            let (code, glyph) = (c as u32, i as u32 + 1);
            // Glyphs follow the characters' order, so the next character continues a run.
            match runs.last_mut() {
                Some((_, end, _)) if *end + 1 == code => *end = code,
                _ => runs.push((code, code, glyph)),
            }
        }
        let mut cmap = Out::default();
        cmap.u16(0).u16(2);
        cmap.u16(0).u16(4).u32(20); // Unicode, full repertoire
        cmap.u16(3).u16(10).u32(20); // Windows, Unicode full repertoire
        cmap.u16(12).u16(0).u32(16 + 12 * runs.len() as u32).u32(0).u32(runs.len() as u32);
        for &(start, end, glyph) in &runs {
            cmap.u32(start).u32(end).u32(glyph);
        }
        let codes: Vec<u16> = chars.iter().map(|&c| c as u32).filter(|&c| c <= 0xFFFF).map(|c| c as u16).collect();

        let first = codes.first().copied().unwrap_or(0x20);
        let last = codes.last().copied().unwrap_or(0x20);
        let average = (glyphs.iter().map(|g| g.advance as u32).sum::<u32>() / count.max(1) as u32) as i16;
        let mut os2 = Out::default();
        os2.u16(4).i16(average).u16(400).u16(5).u16(0);
        for value in [3, 3, 0, 1, 3, 3, 0, 4].map(|p: i16| p * unit as i16) {
            os2.i16(value);
        }
        os2.i16(unit as i16).i16((3 * unit) as i16).i16(0);
        for _ in 0..10 {
            os2.u8(0);
        }
        os2.u32(0b11).u32(0).u32(0).u32(0); // Basic Latin, Latin-1
        os2.0.extend(b"VOXM");
        // Regular, and use the typographic metrics for line spacing.
        os2.u16(0x0040 | 0x0080).u16(first).u16(last);
        os2.i16(ascent).i16(descent).i16(0).u16(ascent as u16).u16(-descent as u16);
        os2.u32(1).u32(0);
        os2.i16((5 * unit) as i16).i16((CAP_HEIGHT * unit) as i16).u16(0).u16(0x20).u16(0);

        let mut post = Out::default();
        post.u32(0x0003_0000).u32(0).i16(-unit as i16).i16(unit as i16).u32(0).u32(0).u32(0).u32(0).u32(0);

        let postscript: String = family.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let names = [(1, family.to_owned()), (2, "Regular".into()), (3, format!("{postscript}-Regular")), (4, family.to_owned()), (5, "Version 1.0".into()), (6, postscript)];
        let mut name = Out::default();
        name.u16(0).u16(names.len() as u16).u16(6 + 12 * names.len() as u16);
        let mut strings = Out::default();
        for (id, text) in &names {
            let utf16: Vec<u16> = text.encode_utf16().collect();
            name.u16(3).u16(1).u16(0x409).u16(*id).u16(utf16.len() as u16 * 2).u16(strings.0.len() as u16);
            for unit in utf16 {
                strings.u16(unit);
            }
        }
        name.0.extend(&strings.0);

        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"OS/2", os2.0),
            (b"cmap", cmap.0),
            (b"glyf", glyf.0),
            (b"head", head.0),
            (b"hhea", hhea.0),
            (b"hmtx", hmtx.0),
            (b"loca", loca.0),
            (b"maxp", maxp.0),
            (b"name", name.0),
            (b"post", post.0),
        ];
        tables.sort_by_key(|(tag, _)| **tag);
        let mut file = Out::default();
        let (range, selector, shift) = search(tables.len() as u16, 16);
        file.u32(0x0001_0000).u16(tables.len() as u16).u16(range).u16(selector).u16(shift);
        let mut offset = 12 + 16 * tables.len() as u32;
        for (tag, data) in &tables {
            file.0.extend(*tag);
            file.u32(checksum(data)).u32(offset).u32(data.len() as u32);
            offset += (data.len() as u32).next_multiple_of(4);
        }
        let head_at = offset_of(&tables, b"head");
        for (_, data) in &tables {
            file.0.extend(data);
            file.pad();
        }
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&file.0));
        file.0[head_at + 8..head_at + 12].copy_from_slice(&adjustment.to_be_bytes());
        file.0
    }

    /// Where a table's data starts in the file.
    fn offset_of(tables: &[(&[u8; 4], Vec<u8>)], tag: &[u8; 4]) -> usize {
        let mut offset = 12 + 16 * tables.len();
        for (t, data) in tables {
            if *t == tag {
                return offset;
            }
            offset += data.len().next_multiple_of(4);
        }
        unreachable!("the table is in the list")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "
// Two letters and an accent.
= o
.....
.....
.###.
#...#
#...#
#...#
.###.
= space 3
mark acute
..#
.#.
compose ó o acute
= O
.###.
#...#
#...#
#...#
#...#
#...#
.###.
compose Ó O acute
";

    #[test]
    fn glyphs_parse_and_compose() {
        let font = PixelFont::parse(SAMPLE).unwrap();
        assert_eq!(font.glyphs[&' '].width, 3);
        let o = &font.glyphs[&'ó'];
        assert_eq!(o.width, 5);
        // The accent sits in the two rows over the x-height.
        assert_eq!(o.first_row(), Some(0));
        assert!(o.rows[0][3] && o.rows[1][2]);
        // Over a capital, it goes above the cap height.
        assert_eq!(font.glyphs[&'Ó'].first_row(), Some(-2));
        assert!(PixelFont::parse("= ab\n#").is_err());
        // Monospace centres narrow glyphs in the cell; includes splice other files in.
        let mono = PixelFont::parse_with("monospace 5\n= i\n#\ninclude more\n", |_| Some("= space 3\n".into())).unwrap();
        assert_eq!((mono.monospace, mono.glyphs[&'i'].width), (Some(5), 5));
        assert!(mono.glyphs[&'i'].rows[0][2]);
        assert_eq!(mono.glyphs[&' '].width, 5);
        assert!(PixelFont::parse("= a\n##\n#").is_err());
    }

    #[test]
    fn outlines_go_around_ink_and_holes() {
        let bitmap = |rows: &[&str]| Bitmap {
            width: rows[0].len() as i32,
            top: 0,
            rows: rows.iter().map(|row| row.chars().map(|c| c == '#').collect()).collect(),
        };
        let area = |c: &Vec<(i32, i32)>| c.iter().zip(c.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum::<i32>() / 2;
        // A ring has one outline outside, clockwise (negative signed area), and one around
        // the hole, counter-clockwise, each with only its four corners.
        let ring = outlines(&bitmap(&["###", "#.#", "###"]));
        let mut areas: Vec<i32> = ring.iter().map(area).collect();
        areas.sort();
        assert_eq!(areas, vec![-9, 1]);
        assert!(ring.iter().all(|c| c.len() == 4));
        // Pixels touching only at a corner, as round letters' do, stay separate outlines.
        let font = PixelFont::parse(SAMPLE).unwrap();
        assert_eq!(outlines(&font.glyphs[&'o']).len(), 4);
        assert_eq!(outlines(&bitmap(&["#.", ".#"])).len(), 2);
    }

    #[test]
    fn the_truetype_file_is_well_formed() {
        let font = PixelFont::parse(SAMPLE).unwrap();
        let ttf = font.to_ttf("Test Pixel");
        assert_eq!(&ttf[..4], &[0, 1, 0, 0]);
        let tables = u16::from_be_bytes([ttf[4], ttf[5]]) as usize;
        assert_eq!(tables, 10);
        // Every table lies within the file, 4-byte aligned, and the whole file sums to the
        // magic number once head's adjustment is in.
        for i in 0..tables {
            let record = &ttf[12 + 16 * i..28 + 16 * i];
            let offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
            let length = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
            assert!(offset % 4 == 0 && offset + length <= ttf.len());
        }
        let sum = ttf.chunks(4).fold(0u32, |s, w| s.wrapping_add(u32::from_be_bytes(w.try_into().unwrap())));
        assert_eq!(sum, 0xB1B0_AFBA);
    }

    #[test]
    fn the_game_fonts_cover_ascii_and_spanish() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/ui/fonts");
        let read = |name: &str| std::fs::read_to_string(format!("{dir}/{name}")).ok();
        for file in ["voxmar.txt", "ledger.txt", "block.txt"] {
            let font = PixelFont::parse_with(&read(file).unwrap(), read).unwrap();
            for c in (' '..='~').chain("áéíóúñÁÉÍÓÚÑ¿¡°×…◆▸◂↔✓•".chars()) {
                assert!(font.glyphs.contains_key(&c), "{file} has no `{c}`");
            }
            // Nothing reaches above the ascent or below the descent the font promises, and a
            // monospace font's glyphs all fit its cell.
            for (c, glyph) in &font.glyphs {
                let rows = glyph.top..glyph.top + glyph.rows.len() as i32;
                assert!(rows.start >= CAP_HEIGHT - ASCENT && rows.end <= CAP_HEIGHT + DESCENT, "{file}: `{c}` overflows");
                if let Some(cell) = font.monospace {
                    assert_eq!(glyph.width, cell, "{file}: `{c}` is not {cell} wide");
                }
            }
        }
    }
}
